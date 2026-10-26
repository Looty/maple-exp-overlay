#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Maple EXP Overlay: a tiny always-on-top EXP/hr overlay for MapleStory Classic World.
//!
//! One exe, a few modes:
//!   (no argument)    the overlay (right-click it, or use the tray icon, for the menu)
//!   --settings       the settings window (the overlay's menu opens it)
//!   --select-area    drag a box around the EXP text (the settings window starts it)
//!   --dump           one look, written to dump.txt in the settings folder, for working out what went wrong
//!   --screen         (with any mode) ignore the game window and use the whole primary screen

#[cfg(not(windows))]
fn main() {
    eprintln!("Maple EXP Overlay runs on Windows only");
}

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod selector;
#[cfg(windows)]
mod settings_ui;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod wgc;

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::UI::HiDpi::*;
    // (the manifest already says per-monitor DPI aware; this covers running without it, e.g. `cargo run`)
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mode = std::env::args().skip(1).find(|a| a.starts_with("--") && a != "--screen");
    match mode.as_deref() {
        Some("--settings") => settings_ui::run(),
        Some("--select-area") => selector::run(),
        Some("--dump") => overlay::dump(),
        _ => overlay::run(),
    }
}
