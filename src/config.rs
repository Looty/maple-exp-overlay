//! Settings: one JSON file in `%APPDATA%\MapleExpOverlay\settings.json`, shared by the overlay and the settings
//! window (which are separate processes: the settings window writes the file, the overlay notices and reloads).
//! Every field has a default (`#[serde(default)]`) so an old or hand-edited file never stops the app from starting,
//! and `clamp()` pulls every value back into its valid range after loading.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

pub const APP_NAME: &str = "Maple EXP Overlay";
pub const DIR_NAME: &str = "MapleExpOverlay";

/// Where the pixels come from. The user picks; nothing switches by itself.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    /// The game window's own picture (Windows Graphics Capture): whatever is on top of the game isn't in it.
    Window,
    /// What is on the screen, like any screen recorder (something on top of the game, such as the taskbar, is in it).
    /// Any other value in an old settings file ends up here too.
    #[default]
    #[serde(other)]
    Screen,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    // ---- overlay look
    pub font: String,
    /// Points.
    pub font_size: i32,
    pub bold: bool,
    /// Whole window opacity, percent.
    pub opacity: i32,
    /// Colours as "#rrggbb".
    pub bg: String,
    pub accent: String,
    pub text: String,
    pub dim: String,
    /// Where the overlay sits, in screen pixels; None = top-left.
    pub pos: Option<[i32; 2]>,
    /// Clicks go through the overlay to the game (it can't be dragged until unlocked here).
    pub locked: bool,
    pub topmost: bool,
    pub taskbar: bool,
    pub tray: bool,
    /// Hide the overlay by itself when no EXP has come in for `hide_after_secs`; it comes back with the next EXP.
    pub hide_idle: bool,
    pub hide_after_secs: i32,

    // ---- what it shows, and where: the items in reading order (see `Item`)
    pub items: Vec<Item>,
    /// 21.1K instead of 21,100.
    pub short_numbers: bool,
    /// Time to next level from the recent rate (the last `window_min` minutes) instead of the whole session's average.
    pub eta_recent: bool,

    // ---- tracking
    /// Seconds without EXP before it counts as paused.
    pub idle_secs: i32,
    /// Length of the "last N minutes" rate.
    pub window_min: i32,
    /// After a break of `new_session_secs` without EXP, the next EXP starts a fresh session: the gained totals and rates
    /// count from that point on.
    pub new_session_idle: bool,
    pub new_session_secs: i32,

    // ---- capture
    /// How the bar is captured, see `CaptureMode`.
    pub capture: CaptureMode,
    /// The box around the EXP text, (x, y, w, h) in screen pixels. The user draws it (Settings > Capture > Select area);
    /// until then nothing is read.
    pub region: Option<[i32; 4]>,
    /// Seconds between looks at the bar.
    pub poll_secs: i32,

    // ---- general
    /// Start with the overlay hidden (tray only).
    pub start_hidden: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font: "Segoe UI".into(),
            font_size: 10,
            bold: true,
            opacity: 100,
            bg: "#181b22".into(),
            accent: "#ffd455".into(),
            text: "#e0e0e0".into(),
            dim: "#a0a0a0".into(),
            pos: None,
            locked: false,
            topmost: true,
            taskbar: true,
            tray: true,
            hide_idle: false,
            hide_after_secs: 60,
            new_session_idle: true,
            new_session_secs: 300,
            items: layout_default(),
            short_numbers: true,
            eta_recent: true,
            idle_secs: 15,
            window_min: 5,
            capture: CaptureMode::Screen,
            region: None,
            poll_secs: 1,
            start_hidden: false,
        }
    }
}

/// One piece of data on the overlay. The overlay lays the shown items out in list order; an item with `newline`
/// starts a new row, the others sit beside the one before it.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Item {
    /// One of `ITEM_IDS`.
    pub id: String,
    pub show: bool,
    pub newline: bool,
    /// 0 = main colour, 1 = normal text, 2 = status colour, 3 and up = a fixed colour from `PALETTE`.
    pub tone: u8,
}

impl Default for Item {
    fn default() -> Self {
        Item { id: String::new(), show: false, newline: true, tone: 1 }
    }
}

pub const ITEM_IDS: [&str; 10] = ["exp_hr", "pct_hr", "exp_window", "eta", "level", "pct", "exp_now", "gained", "gained_pct", "elapsed"];

/// Colours an item can have besides the three theme colours (tones 0 to 2): tone 3 + index.
pub const PALETTE: [&str; 10] =
    ["#ff5a5a", "#ff9a2e", "#ffd455", "#7cfc5a", "#2ee6c9", "#5ac8ff", "#a78bfa", "#ff7ad9", "#ffffff", "#9aa0a6"];
pub const TONES: u8 = 3 + PALETTE.len() as u8;

/// The "#rrggbb" for an item's tone: 0 main colour, 1 normal text, 2 status colour (all from the theme), then `PALETTE`.
pub fn tone_hex(s: &Settings, tone: u8) -> &str {
    match tone {
        0 => &s.accent,
        1 => &s.text,
        2 => &s.dim,
        t => PALETTE.get(t as usize - 3).copied().unwrap_or(&s.text),
    }
}

/// What the settings window calls each item.
pub fn item_name(id: &str) -> &'static str {
    match id {
        "exp_hr" => "EXP per hour",
        "pct_hr" => "% per hour",
        "exp_window" => "Recent EXP rate",
        "eta" => "Time to next level",
        "level" => "Level",
        "pct" => "Current %",
        "exp_now" => "Current EXP",
        "gained" => "EXP gained",
        "gained_pct" => "% gained",
        "elapsed" => "Time farmed",
        _ => "",
    }
}

fn item(id: &str, show: bool, newline: bool, tone: u8) -> Item {
    Item { id: id.into(), show, newline, tone }
}

/// The original look: EXP/h; recent rate + time to level; current %.
pub fn layout_default() -> Vec<Item> {
    vec![
        item("exp_hr", true, true, 0),
        item("pct_hr", false, false, 1),
        item("exp_window", true, true, 1),
        item("eta", true, false, 1),
        item("level", true, true, 1),
        item("pct", true, false, 1),
        item("exp_now", false, false, 1),
        item("gained", false, true, 1),
        item("gained_pct", false, false, 1),
        item("elapsed", false, false, 1),
    ]
}

/// Everything on one row.
pub fn layout_one_line() -> Vec<Item> {
    let mut v = layout_default();
    for (i, it) in v.iter_mut().enumerate() {
        it.newline = i == 0;
    }
    v[1].show = true;
    v
}

/// Every item on its own row.
pub fn layout_list() -> Vec<Item> {
    let mut v = layout_default();
    for it in v.iter_mut() {
        it.newline = true;
        it.show = true;
    }
    v[0].tone = 0;
    v
}

/// A function that builds a layout.
pub type LayoutFn = fn() -> Vec<Item>;

/// Layout presets for the settings window.
pub const LAYOUTS: [(&str, LayoutFn); 3] =
    [("Compact (default)", layout_default), ("One line", layout_one_line), ("Vertical list", layout_list)];

/// "#rrggbb" -> (r, g, b)
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub fn dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join(DIR_NAME))
}

pub fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.json"))
}

/// When the file last changed, so the overlay can tell the settings window saved something.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

impl Settings {
    pub fn load() -> Settings {
        let mut s: Settings =
            path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        s.clamp();
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // write beside it, then rename: the overlay never reads a half-written file
        let tmp = p.with_extension("json.tmp");
        if let Ok(t) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    pub fn clamp(&mut self) {
        let d = Settings::default();
        self.font_size = self.font_size.clamp(7, 40);
        self.opacity = self.opacity.clamp(20, 100);
        self.idle_secs = self.idle_secs.clamp(5, 300);
        self.hide_after_secs = self.hide_after_secs.clamp(5, 3600);
        self.new_session_secs = self.new_session_secs.clamp(30, 7200);
        self.window_min = self.window_min.clamp(1, 60);
        self.poll_secs = self.poll_secs.clamp(1, 10);
        if self.font.trim().is_empty() {
            self.font = d.font;
        }
        for (v, def) in [(&mut self.bg, d.bg), (&mut self.accent, d.accent), (&mut self.text, d.text), (&mut self.dim, d.dim)] {
            if parse_hex(v).is_none() {
                *v = def;
            }
        }
        // every item exactly once, in the saved order; unknown or doubled ones dropped, missing ones added hidden
        let mut seen: Vec<String> = Vec::new();
        let mut items: Vec<Item> = Vec::new();
        for it in std::mem::take(&mut self.items) {
            if ITEM_IDS.contains(&it.id.as_str()) && !seen.contains(&it.id) {
                seen.push(it.id.clone());
                items.push(Item { tone: it.tone.min(TONES - 1), ..it });
            }
        }
        for def in layout_default() {
            if !seen.contains(&def.id) {
                items.push(Item { show: false, newline: true, ..def });
            }
        }
        if let Some(first) = items.first_mut() {
            first.newline = true;
        }
        self.items = items;
        if let Some([_, _, w, h]) = self.region {
            if w < 20 || h < 8 {
                self.region = None;
            }
        }
    }
}

/// Colour presets for the settings window: (name, bg, accent, text, dim).
pub const THEMES: [(&str, &str, &str, &str, &str); 5] = [
    ("Dark (default)", "#181b22", "#ffd455", "#e0e0e0", "#a0a0a0"),
    ("Light", "#f3f4f7", "#b35a00", "#1d2330", "#667085"),
    ("Midnight blue", "#0f1b33", "#5ac8ff", "#dbe7ff", "#8497bd"),
    ("Black", "#000000", "#7CFC5A", "#f2f2f2", "#9a9a9a"),
    ("Maple orange", "#2b1d12", "#ff9a2e", "#ffe9d0", "#c4a27c"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_junk() {
        let mut s: Settings = serde_json::from_str(r#"{"font_size": 900, "bg": "nonsense", "opacity": 5, "region": [1,2,3,4]}"#).unwrap();
        s.clamp();
        let d = Settings::default();
        assert_eq!((s.font_size, s.opacity), (40, 20));
        assert_eq!(s.bg, d.bg);
        assert_eq!(s.region, None);
        assert_eq!(s.accent, d.accent); // missing fields fall back
                                        // an old file that says "auto" (a mode that no longer exists) still loads, as the screen
        let old: Settings = serde_json::from_str(r#"{"capture": "auto", "font_size": 12}"#).unwrap();
        assert_eq!((old.capture, old.font_size), (CaptureMode::Screen, 12));
        let win: Settings = serde_json::from_str(r#"{"capture": "window"}"#).unwrap();
        assert_eq!(win.capture, CaptureMode::Window);
        assert_eq!(parse_hex("#ffd455"), Some((255, 212, 85)));
        assert_eq!(parse_hex("ffd45"), None);
    }
}
