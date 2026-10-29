//! Reads the "EXP. 68,601 [71.68%]" status bar from screenshots and tracks EXP per hour. Platform-neutral:
//! the binary (src/main.rs) does the Win32 side (capture, overlay window, settings window).

pub mod anim;
pub mod config;
pub mod editor;
pub mod exp_table;
pub mod map;
pub mod reader;
pub mod text;
pub mod tracker;
pub mod update;
