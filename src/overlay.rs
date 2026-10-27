//! The overlay: one small always-on-top window, one timer. Plain Win32 + GDI, no UI framework, so it costs next to
//! nothing. Everything about how it looks and what it shows comes from the settings file, which it re-reads
//! whenever the settings window saves.

use crate::capture::{self, game_rect, grab, hash, save_bmp};
use crate::log::dlog;
use crate::system::{self, wide};
use maple_exp::anim::{self, Eased, Pop, Walkers};
use maple_exp::config::{self, parse_hex, CaptureMode, Settings, APP_NAME};
use maple_exp::reader::{diagnose, find_and_read, Reading, Templates};
use maple_exp::text::commas;
use maple_exp::tracker::{Stats, Tracker};

use std::cell::RefCell;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const TIMER_LOOK: usize = 1;
const TIMER_CFG: usize = 2;
const TIMER_ANIM: usize = 3; // runs only while an effect is showing
const ANIM_MS: u32 = 33; // frame time while something is moving fast
const RAINBOW_MS: u32 = 100; // ...and while only the rainbow text is shimmering, which is slower
const POP_CLASS: &str = "MapleExpOverlayPop";
const WALKER_CLASS: &str = "MapleExpOverlayWalker";
const POP_KEY: u32 = 0x00FF00FF; // the pop-up window's see-through colour (magenta)
const SLOW_MS: u32 = 3000; // while no area is selected: just waiting for one
const IDLE_MS: u32 = 5000; // while MapleStory isn't running
const LOST_AFTER: u32 = 5; // failed looks in a row before saying the bar can't be read
const WM_TRAY: u32 = WM_APP + 1;
pub const WM_SHOW: u32 = WM_APP + 2; // another launch of the exe asks this one to open the settings
const WM_UNMINIMIZE: u32 = WM_APP + 3; // posted when Windows minimized the overlay (Win+D, Win+M, a taskbar click)
const ID_SETTINGS: usize = 1;
const ID_RESET: usize = 2;
const ID_TOGGLE: usize = 3;
const ID_QUIT: usize = 4;
const ID_PREVIEW: usize = 5;
pub const CLASS: &str = "MapleExpOverlay";

/// A text colour as an item's tone number (see `config::tone_hex`).
type Tone = u8;
const TONE_STATUS: Tone = 2;
const TONE_RAINBOW: Tone = 250; // not a theme or palette colour: the letters are drawn in a rainbow gradient

/// The effects' state (see `maple_exp::anim`). Does nothing unless one of the `Settings::anim_*` switches is on.
struct Fx {
    /// the animation timer is running, at `ms`
    on: bool,
    ms: u32,
    /// the level is close (see `near_level_up`): the rainbow text and pop-up shimmer
    near: bool,
    /// the crawling snails (one per EXP gain) and the window they are drawn in
    walkers: Walkers,
    walker_hwnd: HWND,
    prev: Option<Reading>,
    pop: Pop,
    /// the pop-up counts percent (the bar shows no EXP number) rather than EXP
    pop_pct: bool,
    pop_text: String,
    pop_hwnd: HWND,
    pop_font: HFONT,
    pop_size: (i32, i32),
    /// rolling values: EXP now, percent, gained EXP, gained percent
    eased: [Eased; 4],
}

struct App {
    s: Settings,
    cfg_mtime: Option<SystemTime>,
    /// Loaded when there is something to read, dropped while the game isn't running.
    templates: Option<Templates>,
    /// MapleStory's window exists.
    game_up: bool,
    tracker: Tracker,
    start: Instant,
    last_hash: u64,
    /// The period the look timer is set to now.
    timer_ms: u32,
    last_reading: Option<Reading>,
    /// what the last failed (or requested) look found, in words ("short|long")
    note: String,
    via: &'static str,
    /// Looks in a row that couldn't read the bar.
    misses: u32,
    status: &'static str,
    rows: Vec<Row>,
    font: HFONT,
    dpi: u32,
    hwnd: HWND,
    /// Hidden from the tray menu.
    hidden: bool,
    /// Hidden by itself for being idle (see `Settings::hide_idle`).
    auto_hidden: bool,
    tray_on: bool,
    size: (i32, i32),
    /// how far up the box sits to make room for the problem or pause message row (0 without one)
    status_shift: i32,
    fx: Fx,
}

thread_local!(static APP: RefCell<Option<App>> = const { RefCell::new(None) });

/// Runs `f` on the app state. None if there is none yet, or if Windows called back into us while it was in use
/// (a window message sent from inside a Win32 call made by `f`): that message is simply skipped.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn rgb(hex: &str) -> u32 {
    let (r, g, b) = parse_hex(hex).unwrap_or((255, 255, 255));
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

// ---------------------------------------------------------------- text

fn short(n: f64) -> String {
    let a = n.abs();
    if a >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", n / 1e6)
    } else if a >= 1e4 {
        format!("{:.1}K", n / 1e3)
    } else {
        commas(n.round().max(0.0) as u64) // 5,140, not 5140
    }
}

fn eta(min: f64) -> String {
    if min >= 600.0 {
        format!("{:.0} h", min / 60.0)
    } else if min >= 60.0 {
        format!("{}h {:02}m", (min / 60.0) as u32, (min % 60.0) as u32)
    } else if min < 1.0 {
        "<1 min".to_string()
    } else {
        format!("{:.0} min", min)
    }
}

fn clock(secs: f64) -> String {
    let t = secs as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

type Row = Vec<(String, Tone)>;

/// The next level is less than `anim_rainbow_min` minutes away (and farming hasn't paused).
fn near_level_up(s: &Settings, st: &Stats) -> bool {
    s.anim_rainbow && !st.paused && eta_minutes(s, st).is_some_and(|m| m < s.anim_rainbow_min as f64)
}

/// Minutes to the next level, by the rate the settings say to use.
fn eta_minutes(s: &Settings, st: &Stats) -> Option<f64> {
    if s.eta_recent {
        st.eta_recent_min.or(st.eta_min)
    } else {
        st.eta_min
    }
}

/// The text of one item right now; None when there is nothing to say yet (it is then left out).
fn item_text(id: &str, s: &Settings, st: &Stats, problem: bool) -> Option<String> {
    let num = |n: f64| if s.short_numbers { short(n) } else { commas(n.round() as u64) };
    match id {
        "exp_hr" => Some(match (st.exp_hr, st.pct_hr) {
            (Some(e), _) => format!("EXP/h  {}", num(e)),
            (None, Some(_)) if st.exp_now.is_some() => "EXP/h  0".to_string(), // the number is on the bar, no gain yet
            (None, Some(p)) => format!("EXP/h  {:.1}%", p),
            _ => "EXP/h  ...".to_string(),
        }),
        "pct_hr" => st.pct_hr.map(|p| format!("{:.1} %/h", p)),
        "exp_window" => st.exp_5m.map(|e| format!("{} min  {}", s.window_min, num(e))),
        "eta" => eta_minutes(s, st).map(|m| format!("Lv in {}", eta(m))),
        // (a problem or pause message takes the percentage's place, as the status line)
        "level" => st.level.map(|l| format!("Lv {l}")),
        "pct" if !problem => st.pct.map(|p| format!("{:.2}%", p)),
        "exp_now" => st.exp_now.map(|e| format!("EXP {}", commas(e))),
        "gained" => st.gained_exp.map(|g| format!("Gained {}", num(g))),
        "gained_pct" => st.pct.map(|_| format!("Gained {:.2}%", st.gained_pct)),
        "elapsed" => Some(format!("Time {}", clock(st.active))),
        _ => None,
    }
}

/// The overlay's rows, from the items the settings say to show, in their order: an item with `newline` starts a row.
fn build_rows(s: &Settings, st: &Stats, status: &str) -> Vec<Row> {
    let problem = if !status.is_empty() {
        Some(status.to_string())
    } else if st.paused {
        Some("paused (no EXP)".to_string())
    } else {
        None
    };
    // close to the next level: the percent, level and time to level get a rainbow gradient
    let near = near_level_up(s, st);
    let mut rows: Vec<Row> = Vec::new();
    for it in s.items.iter().filter(|i| i.show) {
        let Some(text) = item_text(&it.id, s, st, problem.is_some()) else { continue };
        if it.newline || rows.is_empty() {
            rows.push(Vec::new());
        }
        let tone = if near && matches!(it.id.as_str(), "pct" | "level" | "eta") { TONE_RAINBOW } else { it.tone };
        rows.last_mut().unwrap().push((text, tone));
    }
    if let Some(p) = problem {
        rows.insert(0, vec![(p, TONE_STATUS)]); // above the items, so the layout below keeps its place
    }
    if rows.is_empty() {
        rows.push(vec![(APP_NAME.to_string(), TONE_STATUS)]); // everything switched off: never an empty box
    }
    rows
}

/// Hands the pages this process isn't using right now back to Windows (what Task Manager shows as its memory).
unsafe fn trim_memory() {
    windows_sys::Win32::System::Threading::SetProcessWorkingSetSize(
        windows_sys::Win32::System::Threading::GetCurrentProcess(),
        usize::MAX,
        usize::MAX,
    );
}

unsafe fn text_width(dc: HDC, text: &str) -> i32 {
    let t: Vec<u16> = text.encode_utf16().collect();
    let mut sz: SIZE = zeroed();
    GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32, &mut sz);
    sz.cx
}

// ---------------------------------------------------------------- one look

impl App {
    /// One look at the EXP bar, in the box the user selected. Nothing is searched for: no box, nothing read.
    unsafe fn look(&mut self, force_save: bool) {
        let now = self.start.elapsed().as_secs_f64();
        let Some([x, y, cw, ch]) = self.s.region else {
            self.status = "Set the EXP area: right-click > Settings";
            self.tracker.observe(now, None);
            return;
        };
        // Is the game there? Looking for its window takes microseconds. Without it the overlay idles: a look every few seconds
        // (not every second), nothing kept in memory between looks, and nothing written to disk.
        let up = capture::game_running();
        if up != self.game_up {
            self.game_up = up;
            self.last_hash = 0;
            self.set_look_timer();
            dlog!("game_up -> {up}");
        }
        if !up && !force_save && !capture::screen_forced() {
            // no game, nothing on screen to read: skip the capture, the hash and the search altogether
            dlog!("look: game not running, skipped");
            self.last_hash = 0;
            self.last_reading = None;
            self.status = "Waiting for MapleStory...";
            self.tracker.observe(now, None);
            self.templates = None;
            trim_memory();
            return;
        }
        let t0 = Instant::now();
        let (frame, via) = grab(self.s.capture == CaptureMode::Window, x, y, cw, ch);
        let grab_us = t0.elapsed().as_micros();
        if via != self.via {
            self.via = via;
            self.last_hash = 0;
        }
        let reading = match frame {
            Some(f) => {
                let hsh = hash(&f);
                if hsh == self.last_hash && !force_save {
                    self.last_reading // the bar hasn't changed: nothing to read
                } else {
                    self.last_hash = hsh;
                    // (the digit templates, about 0.6 MB, are loaded only when there is a picture to read)
                    let t1 = Instant::now();
                    let templates = self.templates.get_or_insert_with(Templates::builtin);
                    let r = find_and_read(templates, &f).map(|(_, r)| r);
                    dlog!("look: {}x{} via {via}, grab {grab_us}us, read {}us -> {r:?}", f.w, f.h, t1.elapsed().as_micros());
                    self.last_reading = r;
                    if (r.is_none() && self.game_up) || force_save {
                        // keep what was seen, so a failure can be looked at
                        let templates = self.templates.get_or_insert_with(Templates::builtin);
                        self.note = format!("{} [captured from: {}]", diagnose(templates, &f), self.via);
                        if let Some(dir) = config::dir() {
                            let _ = std::fs::create_dir_all(&dir);
                            save_bmp(&dir.join("last-capture.bmp"), &f);
                            let long = self.note.split_once('|').map_or(self.note.as_str(), |(_, l)| l);
                            let _ = std::fs::write(dir.join("last-capture.txt"), long);
                        }
                    }
                    r
                }
            }
            None => None,
        };
        // (the time right now, not the `now` from before the read: a slow read would otherwise age the effects before they start)
        self.note_gain(reading, self.start.elapsed().as_secs_f64());
        if reading.is_some() {
            self.misses = 0;
            self.status = "";
        } else if !self.game_up {
            self.status = "Waiting for MapleStory...";
        } else {
            self.misses += 1;
            if self.misses >= LOST_AFTER {
                self.status = "Can't read the EXP bar"; // (the short reason from the last look is shown instead, if there is one)
            }
        }
        self.tracker.observe(now, reading);
        if !self.game_up {
            self.templates = None;
            trim_memory(); // give back what the pages touched by this look used: an idle overlay stays tiny
        }
        self.set_look_timer();
    }

    /// How long to wait before the next look.
    fn look_ms(&self) -> u32 {
        // no area selected: nothing to look at, only wait for the box being set; no game: look rarely
        if self.s.region.is_none() {
            SLOW_MS
        } else if !self.game_up {
            IDLE_MS
        } else {
            self.s.poll_ms as u32 // exactly what the settings say: EXP shows up within one look
        }
    }

    /// (Re)starts the look timer at the rate that fits now; does nothing when it already runs at that rate.
    unsafe fn set_look_timer(&mut self) {
        let ms = self.look_ms();
        if ms != self.timer_ms {
            self.timer_ms = ms;
            SetTimer(self.hwnd, TIMER_LOOK, ms, None);
            dlog!("look timer -> {ms}ms");
        }
    }

    // ------------------------------------------------------------ effects

    /// Compares this reading with the last one: EXP that came in starts a snail and the pop-up.
    unsafe fn note_gain(&mut self, reading: Option<Reading>, now: f64) {
        let Some(r) = reading else { return };
        let prev = self.fx.prev.replace(r);
        let Some(p) = prev.filter(|_| self.s.anim_pop || self.s.anim_snail) else { return };
        let gain = match (r.exp, p.exp) {
            (Some(e), Some(pe)) if e > pe => Some(((e - pe) as f64, false)),
            (None, None) if r.pct > p.pct => Some((r.pct - p.pct, true)),
            _ => None, // no gain (or the level went up: the number went down)
        };
        let Some((amount, pct)) = gain else { return };
        dlog!("fx: gain {amount} (percent: {pct})");
        if self.s.anim_snail {
            self.fx.walkers.spawn(now);
        }
        if self.s.anim_pop {
            if self.fx.pop_pct != pct {
                self.fx.pop = Pop::default(); // percent and EXP don't add up
            }
            self.fx.pop_pct = pct;
            self.fx.pop.add(amount, now);
        }
        self.start_fx();
    }

    /// The menu's "Preview effects", for seeing the effects without waiting for a kill: one pretend gain. Only the effects
    /// switched on in the settings show.
    unsafe fn preview_fx(&mut self) {
        let now = self.start.elapsed().as_secs_f64();
        if self.s.anim_snail {
            self.fx.walkers.spawn(now);
        }
        if self.s.anim_pop {
            self.fx.pop_pct = false;
            self.fx.pop.add(1234.0, now);
        }
        self.start_fx();
    }

    /// Runs the effect timer at `ms` (starts it, or changes its speed).
    unsafe fn set_fx_timer(&mut self, ms: u32) {
        if !self.fx.on || self.fx.ms != ms {
            self.fx.on = true;
            self.fx.ms = ms;
            SetTimer(self.hwnd, TIMER_ANIM, ms, None);
            dlog!("fx: timer -> {ms}ms");
        }
    }

    unsafe fn start_fx(&mut self) {
        self.set_fx_timer(ANIM_MS);
    }

    /// Something is moving (rolling, floating, crawling).
    fn fx_busy(&self, now: f64) -> bool {
        self.fx.eased.iter().any(|e| e.busy(now)) || self.fx.pop.alive(now) || self.fx.walkers.alive(now)
    }

    /// Shows rolling numbers instead of the real ones.
    unsafe fn fx_apply(&mut self, st: &mut Stats, now: f64) {
        if !self.s.anim_count {
            return;
        }
        let e = &mut self.fx.eased;
        st.exp_now = st.exp_now.map(|v| e[0].at(v as f64, now).round() as u64);
        st.pct = st.pct.map(|v| e[1].at(v, now));
        st.gained_exp = st.gained_exp.map(|v| e[2].at(v, now));
        st.gained_pct = e[3].at(st.gained_pct, now);
        if e.iter().any(|x| x.busy(now)) {
            self.start_fx();
        }
    }

    /// One frame of the effects; stops the timer when nothing is showing any more.
    unsafe fn fx_tick(&mut self) {
        let now = self.start.elapsed().as_secs_f64();
        self.refresh();
        self.update_pop(now);
        self.update_walkers(now);
        if self.fx_busy(now) {
            self.set_fx_timer(ANIM_MS);
        } else if self.fx.near {
            self.set_fx_timer(RAINBOW_MS);
        } else {
            KillTimer(self.hwnd, TIMER_ANIM);
            dlog!("fx: done");
            self.fx.on = false;
            self.hide_pop();
        }
    }

    /// Moves the "+EXP" pop-up (its own little see-through window) just above the overlay, floating up and fading.
    unsafe fn update_pop(&mut self, now: f64) {
        let shown = self.fx.pop.look(now).filter(|_| !self.hidden && !self.auto_hidden);
        let Some((alpha, rise)) = shown else {
            self.hide_pop();
            return;
        };
        if self.fx.pop_hwnd.is_null() {
            self.fx.pop_hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                wide(POP_CLASS).as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if self.fx.pop_hwnd.is_null() {
                return;
            }
        }
        let pop = self.fx.pop_hwnd;
        let sum = self.fx.pop.sum;
        let text = if self.fx.pop_pct {
            format!("+{sum:.2}%")
        } else if self.s.short_numbers {
            format!("+{}", short(sum))
        } else {
            format!("+{}", commas(sum.round() as u64))
        };
        if text != self.fx.pop_text {
            let dc = GetDC(self.hwnd);
            let old = SelectObject(dc, self.fx.pop_font);
            let mut tm: TEXTMETRICW = zeroed();
            GetTextMetricsW(dc, &mut tm);
            self.fx.pop_size = (text_width(dc, &text) + 4, tm.tmHeight + 2);
            SelectObject(dc, old);
            ReleaseDC(self.hwnd, dc);
            self.fx.pop_text = text;
            InvalidateRect(pop, null(), 0);
        } else if self.fx.near {
            InvalidateRect(pop, null(), 0); // same text, but the rainbow colours move every frame
        }
        let (w, h) = self.fx.pop_size;
        let mut r: RECT = zeroed();
        GetWindowRect(self.hwnd, &mut r);
        let scale = self.dpi as f64 / 96.0;
        let lift = (rise * 22.0 * scale) as i32;
        // above the overlay (and the snail strip, if that is on), or below it when it sits at the top of the screen
        let strip = self.strip_height(now);
        let y = if r.top >= h + strip + (30.0 * scale) as i32 { r.top - h - 2 - lift - strip } else { r.bottom + 2 + lift + strip };
        SetLayeredWindowAttributes(pop, POP_KEY, alpha, LWA_COLORKEY | LWA_ALPHA);
        SetWindowPos(pop, HWND_TOPMOST, r.left + (8.0 * scale) as i32, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    unsafe fn hide_pop(&mut self) {
        if !self.fx.pop_hwnd.is_null() {
            ShowWindow(self.fx.pop_hwnd, SW_HIDE);
        }
        self.fx.pop_text.clear();
    }

    unsafe fn paint_pop(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r: RECT = zeroed();
        GetClientRect(hwnd, &mut r);
        let key = CreateSolidBrush(POP_KEY);
        FillRect(dc, &r, key);
        DeleteObject(key);
        SetBkMode(dc, TRANSPARENT as i32);
        let old = SelectObject(dc, self.fx.pop_font);
        if self.fx.near {
            // close to the next level: the +EXP shimmers in the same rainbow as the text
            let (n, phase) = (self.fx.pop_text.chars().count().max(1) as f64, self.start.elapsed().as_secs_f64() * 0.4);
            let mut x = 2;
            for (k, ch) in self.fx.pop_text.chars().enumerate() {
                SetTextColor(dc, anim::rainbow(k as f64 / n * 0.85 + phase));
                let s = ch.to_string();
                let t: Vec<u16> = s.encode_utf16().collect();
                TextOutW(dc, x, 1, t.as_ptr(), t.len() as i32);
                x += text_width(dc, &s);
            }
        } else {
            SetTextColor(dc, rgb(&self.s.accent));
            let t: Vec<u16> = self.fx.pop_text.encode_utf16().collect();
            TextOutW(dc, 2, 1, t.as_ptr(), t.len() as i32);
        }
        SelectObject(dc, old);
        EndPaint(hwnd, &ps);
    }

    // ------------------------------------------------------------ look and feel

    unsafe fn make_font(&mut self) {
        if !self.font.is_null() {
            DeleteObject(self.font);
        }
        let height = -(self.s.font_size * self.dpi as i32 / 72);
        let weight = if self.s.bold { 700 } else { 400 };
        self.font = CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(&self.s.font).as_ptr());
        // the pop-up is drawn on a see-through colour key, where smoothed text edges would show as fringes
        if !self.fx.pop_font.is_null() {
            DeleteObject(self.fx.pop_font);
        }
        self.fx.pop_font = CreateFontW(height, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 3, 0, wide(&self.s.font).as_ptr());
    }

    /// Everything the settings control, applied to the live window.
    unsafe fn apply(&mut self) {
        let s = self.s.clone();
        self.tracker.idle = s.idle_secs as f64;
        self.tracker.window = s.window_min as f64 * 60.0;
        self.tracker.new_session_after = s.new_session_idle.then_some(s.new_session_secs as f64);
        self.dpi = GetDpiForWindow(self.hwnd).max(96);
        self.make_font();
        SetLayeredWindowAttributes(self.hwnd, 0, (s.opacity * 255 / 100) as u8, LWA_ALPHA);
        SetWindowPos(
            self.hwnd,
            if s.topmost { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );

        // a taskbar button or not: the style only takes effect on a hidden-then-shown window
        let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
        let want = (ex & !(WS_EX_APPWINDOW | WS_EX_TOOLWINDOW)) | if s.taskbar { WS_EX_APPWINDOW } else { WS_EX_TOOLWINDOW };
        if want != ex {
            ShowWindow(self.hwnd, SW_HIDE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, want as isize);
            self.sync_visibility();
        }

        // the tray icon is the way back to the settings when the overlay is locked, so hiding needs it
        if s.tray && !self.tray_on {
            add_tray_icon(self.hwnd);
            self.tray_on = true;
        } else if !s.tray && self.tray_on {
            remove_tray_icon(self.hwnd);
            self.tray_on = false;
        }

        if let Some([x, y]) = s.pos {
            let mut r: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut r);
            let y = y - self.status_shift;
            if (r.left, r.top) != (x, y) {
                SetWindowPos(self.hwnd, null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        self.misses = 0;
        self.set_look_timer();
        self.last_hash = 0;
        self.rows.clear(); // force a redraw and resize
        self.refresh();
    }

    /// Rebuilds the text, resizes the window to fit it and repaints, if anything changed.
    unsafe fn refresh(&mut self) {
        let dpi = GetDpiForWindow(self.hwnd).max(96);
        if dpi != self.dpi {
            self.dpi = dpi;
            self.make_font();
            self.rows.clear();
        }
        let now = self.start.elapsed().as_secs_f64();
        let mut st = self.tracker.stats(now);
        if self.s.anim_any() {
            self.fx_apply(&mut st, now);
        }
        // hide by itself when nothing has come in for a while, and come back with the next EXP
        let idle_for = st.since_gain.unwrap_or(now);
        let want_hidden = self.s.hide_idle && idle_for >= self.s.hide_after_secs as f64;
        if want_hidden != self.auto_hidden {
            self.auto_hidden = want_hidden;
            self.sync_visibility();
        }
        let short_note = self.note.split('|').next().unwrap_or("");
        let status = if !self.status.is_empty() && !short_note.is_empty() { short_note } else { self.status };
        // close to the next level: the rainbow text shimmers, which keeps a slow timer running
        let has_status = !status.is_empty() || st.paused;
        let near = near_level_up(&self.s, &st);
        if near != self.fx.near {
            dlog!("fx: near level-up {near} (eta {:?} min, paused {})", eta_minutes(&self.s, &st), st.paused);
        }
        self.fx.near = near;
        let rows = build_rows(&self.s, &st, status);
        if self.fx.near && !self.fx.on {
            self.set_fx_timer(RAINBOW_MS);
        }
        if rows == self.rows {
            if self.fx.on {
                InvalidateRect(self.hwnd, null(), 0); // same text, but the colours are moving
            }
            return;
        }
        self.rows = rows;

        let dc = GetDC(self.hwnd);
        let old = SelectObject(dc, self.font);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(dc, &mut tm);
        let gap = tm.tmAveCharWidth * 3;
        let mut widest = 0;
        for row in &self.rows {
            let widths: i32 = row.iter().map(|(text, _)| text_width(dc, text)).sum();
            widest = widest.max(widths + gap * (row.len() as i32 - 1));
        }
        SelectObject(dc, old);
        ReleaseDC(self.hwnd, dc);
        let pad = 8 * self.dpi as i32 / 96;
        let line = tm.tmHeight + 2 * self.dpi as i32 / 96;
        let size = (widest + 2 * pad, self.rows.len() as i32 * line + 2 * pad - self.dpi as i32 / 96 * 2);
        // A problem or pause message adds a row on top. The box then grows upwards, its bottom edge staying where it is:
        // growing down would cover the EXP bar the overlay is reading, and the covered bar is then what the message
        // complains about. `status_shift` is how far up the box sits for that row, so the saved position stays the one
        // without it.
        let want_shift = if has_status { line } else { 0 };
        let dy = want_shift - self.status_shift;
        if size != self.size || dy != 0 {
            self.size = size;
            let mut r: RECT = zeroed();
            GetWindowRect(self.hwnd, &mut r);
            SetWindowPos(self.hwnd, null_mut(), r.left, r.top - dy, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE);
            self.status_shift = want_shift;
        }
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r: RECT = zeroed();
        GetClientRect(hwnd, &mut r);
        // drawn off-screen first, so repainting never flickers
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
        let old_bmp = SelectObject(mem, bmp);
        let bg = CreateSolidBrush(rgb(&self.s.bg));
        FillRect(mem, &r, bg);
        DeleteObject(bg);
        SetBkMode(mem, TRANSPARENT as i32);
        let old_font = SelectObject(mem, self.font);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(mem, &mut tm);
        let line = tm.tmHeight + 2 * self.dpi as i32 / 96;
        let pad = 8 * self.dpi as i32 / 96;
        let gap = tm.tmAveCharWidth * 3;
        for (i, row) in self.rows.iter().enumerate() {
            let mut x = pad;
            let y = pad - self.dpi as i32 / 96 * 2 + i as i32 * line;
            for (text, tone) in row {
                if *tone == TONE_RAINBOW {
                    // one letter at a time, each its own colour along the wheel (red to violet)
                    let n = text.chars().count().max(1) as f64;
                    let phase = self.start.elapsed().as_secs_f64() * 0.4; // the colours drift along the letters
                    for (k, ch) in text.chars().enumerate() {
                        SetTextColor(mem, anim::rainbow(k as f64 / n * 0.85 + phase));
                        let s = ch.to_string();
                        let t: Vec<u16> = s.encode_utf16().collect();
                        TextOutW(mem, x, y, t.as_ptr(), t.len() as i32);
                        x += text_width(mem, &s);
                    }
                    x += gap;
                    continue;
                }
                SetTextColor(mem, rgb(config::tone_hex(&self.s, *tone)));
                let t: Vec<u16> = text.encode_utf16().collect();
                TextOutW(mem, x, y, t.as_ptr(), t.len() as i32);
                x += text_width(mem, text) + gap;
            }
        }
        SelectObject(mem, old_font);
        BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }

    /// The snails: a see-through strip as wide as the overlay, standing on its top edge (or hanging under it when there
    /// is no room above), that the crawling snails are drawn into.
    unsafe fn update_walkers(&mut self, now: f64) {
        let walking = self.fx.walkers.alive(now) && !self.hidden && !self.auto_hidden;
        if !walking {
            if !self.fx.walker_hwnd.is_null() {
                ShowWindow(self.fx.walker_hwnd, SW_HIDE);
            }
            return;
        }
        if self.fx.walker_hwnd.is_null() {
            self.fx.walker_hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                wide(WALKER_CLASS).as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if self.fx.walker_hwnd.is_null() {
                return;
            }
            SetLayeredWindowAttributes(self.fx.walker_hwnd, POP_KEY, 255, LWA_COLORKEY);
        }
        let mut r: RECT = zeroed();
        GetWindowRect(self.hwnd, &mut r);
        let sh = self.strip_height(now);
        let y = if r.top >= sh { r.top - sh } else { r.bottom };
        SetWindowPos(self.fx.walker_hwnd, HWND_TOPMOST, r.left, y, r.right - r.left, sh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(self.fx.walker_hwnd, null(), 0);
    }

    /// Half screen pixels per sprite pixel (see `anim::cell_start`).
    fn walker_px(&self) -> i32 {
        (anim::SNAIL_PX * self.dpi as i32 / 96).max(1)
    }

    /// How tall the snail strip is right now: a snail's height while any is crawling, else 0.
    fn strip_height(&self, now: f64) -> i32 {
        if self.fx.walkers.alive(now) {
            anim::cell_start(anim::SPRITE_H, self.walker_px())
        } else {
            0
        }
    }

    unsafe fn paint_walkers(&self, hwnd: HWND) {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r: RECT = zeroed();
        GetClientRect(hwnd, &mut r);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
        let old_bmp = SelectObject(mem, bmp);
        let key = CreateSolidBrush(POP_KEY);
        FillRect(mem, &r, key);
        DeleteObject(key);
        let old = SelectObject(mem, GetStockObject(DC_BRUSH));
        let now = self.start.elapsed().as_secs_f64();
        let frame = (now * 3.0) as usize % 2; // the eye stalks and the foot move
        let px = self.walker_px();
        let sw = anim::cell_start(anim::SPRITE_W, px);
        let y0 = r.bottom - anim::cell_start(anim::SPRITE_H, px); // standing on the bottom of the strip
        for t in self.fx.walkers.progress(now) {
            let x0 = r.right - ((r.right + sw) as f64 * t) as i32;
            for (row, line) in anim::SPRITE[frame].iter().enumerate() {
                for (col, c) in line.bytes().enumerate() {
                    let Some(color) = anim::sprite_color(c) else { continue };
                    SetDCBrushColor(mem, color);
                    let (l, t) = (anim::cell_start(col, px), anim::cell_start(row, px));
                    let (w, h) = (anim::cell_start(col + 1, px) - l, anim::cell_start(row + 1, px) - t);
                    PatBlt(mem, x0 + l, y0 + t, w, h, PATCOPY);
                }
            }
        }
        SelectObject(mem, old);
        BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }

    /// Shown unless the user hid it (tray) or it hid itself for being idle.
    unsafe fn sync_visibility(&self) {
        if self.hidden || self.auto_hidden {
            ShowWindow(self.hwnd, SW_HIDE);
        } else {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(
                self.hwnd,
                if self.s.topmost { HWND_TOPMOST } else { HWND_NOTOPMOST },
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// While the game is up, puts the overlay back if Windows took it away: minimized (Win+D, Win+M, a taskbar click
    /// with "Taskbar button" on), or pushed under the game when the game window came to the front (a game window
    /// that is itself topmost lands above us). Never shows a window the user or idle-hiding hid.
    unsafe fn keep_shown(&self) {
        if !self.game_up || self.hidden || self.auto_hidden {
            return;
        }
        if IsIconic(self.hwnd) != 0 {
            dlog!("overlay was minimized: restoring");
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        // only while the game is in front, so the settings window and the area picker can still sit above the overlay
        if self.s.topmost && capture::game_in_front() {
            SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Re-reads the settings file if the settings window saved a change.
    unsafe fn check_config(&mut self) {
        let m = config::modified();
        if m != self.cfg_mtime {
            self.cfg_mtime = m;
            self.s = Settings::load();
            self.apply();
        }
    }
}

// ---------------------------------------------------------------- tray, menu, window

unsafe fn app_icon() -> HICON {
    // the exe's embedded icon (assets/icon.ico): resource id 1, which the API takes as a pointer-sized number
    let icon = LoadIconW(GetModuleHandleW(null()), std::ptr::without_provenance::<u16>(1));
    if icon.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        icon
    }
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = app_icon();
    for (i, c) in APP_NAME.encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

pub fn open_settings() {
    // let the settings process take the foreground (over a borderless-fullscreen game too)
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX) };
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--settings").spawn();
    }
}

unsafe fn toggle_visible() {
    with_app(|app| {
        app.hidden = !app.hidden;
        if !app.hidden {
            app.auto_hidden = false; // asked to see it: show it now, even if it was hidden for being idle
        }
        app.sync_visibility();
    });
}

unsafe fn show_menu(hwnd: HWND) {
    let hidden = with_app(|a| a.hidden).unwrap_or(false);
    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, ID_SETTINGS, wide("Settings...").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_RESET, wide("Reset session").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_PREVIEW, wide("Preview effects").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_TOGGLE, wide(if hidden { "Show overlay" } else { "Hide overlay" }).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_QUIT, wide("Quit").as_ptr());
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    SetForegroundWindow(hwnd); // (or the menu doesn't close when clicking elsewhere)
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, p.x, p.y, 0, hwnd, null()) as usize;
    DestroyMenu(menu);
    match cmd {
        ID_SETTINGS => open_settings(),
        ID_RESET => {
            with_app(|app| {
                app.tracker.reset();
                app.refresh();
            });
        }
        ID_PREVIEW => {
            with_app(|app| app.preview_fx());
        }
        ID_TOGGLE => toggle_visible(),
        ID_QUIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn save_pos(hwnd: HWND) {
    let mut r: RECT = zeroed();
    GetWindowRect(hwnd, &mut r);
    let mut s = Settings::load();
    let shift = with_app(|app| app.status_shift).unwrap_or(0); // (the saved position is the one without the message row)
    s.pos = Some([r.left, r.top + shift]);
    s.save();
    with_app(|app| {
        app.s.pos = s.pos;
        app.cfg_mtime = config::modified(); // our own write isn't news
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                with_app(|app| {
                    if wp == TIMER_LOOK {
                        app.look(false);
                        app.refresh();
                        app.keep_shown();
                    } else if wp == TIMER_CFG {
                        app.check_config();
                    } else if wp == TIMER_ANIM {
                        app.fx_tick();
                    }
                });
                0
            }
            WM_PAINT => {
                let painted = APP.with(|a| match a.try_borrow().ok().as_deref() {
                    Some(Some(app)) => {
                        app.paint(hwnd);
                        true
                    }
                    _ => false,
                });
                if !painted {
                    let mut ps: PAINTSTRUCT = zeroed();
                    BeginPaint(hwnd, &mut ps);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => {
                if with_app(|a| a.s.locked).unwrap_or(false) {
                    -1 // HTTRANSPARENT: clicks go through to the game
                } else {
                    HTCAPTION as LRESULT // the whole window drags
                }
            }
            WM_NCRBUTTONUP => {
                show_menu(hwnd);
                0
            }
            WM_NCLBUTTONDBLCLK => {
                with_app(|app| {
                    app.tracker.reset();
                    app.refresh();
                });
                0
            }
            WM_TRAY => {
                match lp as u32 {
                    WM_RBUTTONUP => show_menu(hwnd),
                    WM_LBUTTONUP => toggle_visible(),
                    WM_LBUTTONDBLCLK => open_settings(),
                    _ => {}
                }
                0
            }
            WM_SHOW => {
                with_app(|app| {
                    if app.hidden || app.auto_hidden {
                        app.hidden = false;
                        app.auto_hidden = false;
                        app.sync_visibility();
                    }
                });
                open_settings();
                0
            }
            // the overlay is never minimized: with a taskbar button, Win+D, Win+M or a click on that button would
            // otherwise make it vanish until restarted
            WM_SYSCOMMAND if (wp & 0xFFF0) as u32 == SC_MINIMIZE => 0,
            WM_SIZE => {
                if wp as u32 == SIZE_MINIMIZED {
                    PostMessageW(hwnd, WM_UNMINIMIZE, 0, 0); // (not from inside the minimize itself)
                }
                0
            }
            WM_UNMINIMIZE => {
                if IsIconic(hwnd) != 0 && !with_app(|a| a.hidden || a.auto_hidden).unwrap_or(false) {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                0
            }
            WM_EXITSIZEMOVE => {
                save_pos(hwnd);
                0
            }
            WM_DESTROY => {
                with_app(|app| {
                    if app.tray_on {
                        remove_tray_icon(hwnd);
                    }
                });
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The "+EXP" pop-up window: a see-through, click-through strip of text (see `App::update_pop`).
unsafe extern "system" fn pop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let painted = APP.with(|a| match a.try_borrow().ok().as_deref() {
                    Some(Some(app)) => {
                        app.paint_pop(hwnd);
                        true
                    }
                    _ => false,
                });
                if !painted {
                    let mut ps: PAINTSTRUCT = zeroed();
                    BeginPaint(hwnd, &mut ps);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => -1, // HTTRANSPARENT
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The snails' window: see-through and click-through (see `App::update_walkers`).
unsafe extern "system" fn walker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let painted = APP.with(|a| match a.try_borrow().ok().as_deref() {
                    Some(Some(app)) => {
                        app.paint_walkers(hwnd);
                        true
                    }
                    _ => false,
                });
                if !painted {
                    let mut ps: PAINTSTRUCT = zeroed();
                    BeginPaint(hwnd, &mut ps);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => -1, // HTTRANSPARENT
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Another copy is already running: ask it to open the settings (so launching the exe again is never a dead end).
unsafe fn poke_running_copy() {
    let h = FindWindowW(wide(CLASS).as_ptr(), null());
    if !h.is_null() {
        PostMessageW(h, WM_SHOW, 0, 0);
    }
}

pub fn run() {
    unsafe {
        if !system::claim("Local\\MapleExpOverlay.Instance") {
            poke_running_copy();
            return;
        }
        system::refresh_autostart();
        let s = Settings::load();
        let hinst = GetModuleHandleW(null());
        let class = wide(CLASS);
        let icon = app_icon();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: icon,
            ..zeroed()
        };
        RegisterClassW(&wc);
        let pop_class = wide(POP_CLASS);
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(pop_proc), hInstance: hinst, lpszClassName: pop_class.as_ptr(), ..zeroed() });
        let walker_class = wide(WALKER_CLASS);
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(walker_proc), hInstance: hinst, lpszClassName: walker_class.as_ptr(), ..zeroed() });
        let (x, y) = s.pos.map_or((24, 24), |p| (p[0], p[1]));
        let (x, y) = on_screen(x, y);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_APPWINDOW,
            class.as_ptr(),
            wide(APP_NAME).as_ptr(),
            WS_POPUP,
            x,
            y,
            200,
            66,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
        let hidden = s.start_hidden && s.tray;
        let mtime = config::modified();
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                s,
                cfg_mtime: mtime,
                templates: None,
                game_up: false,
                tracker: Tracker::default(),
                start: Instant::now(),
                last_hash: 0,
                timer_ms: 0,
                last_reading: None,
                note: String::new(),
                via: "",
                misses: 0,
                status: "starting...",
                rows: Vec::new(),
                font: null_mut(),
                dpi: 96,
                hwnd,
                hidden,
                auto_hidden: false,
                tray_on: false,
                size: (0, 0),
                status_shift: 0,
                fx: Fx {
                    on: false,
                    ms: 0,
                    near: false,
                    walkers: Walkers::default(),
                    walker_hwnd: null_mut(),
                    prev: None,
                    pop: Pop::default(),
                    pop_pct: false,
                    pop_text: String::new(),
                    pop_hwnd: null_mut(),
                    pop_font: null_mut(),
                    pop_size: (0, 0),
                    eased: [Eased::default(); 4],
                },
            })
        });
        with_app(|app| app.apply());
        if !hidden {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        SetTimer(hwnd, TIMER_CFG, 1000, None);
        // nothing is read until the user has drawn a box around the EXP text: take them straight to the place to do that
        if with_app(|app| app.s.region.is_none()).unwrap_or(false) && !hidden {
            open_settings();
        }
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// A remembered spot that's no longer on any screen (an unplugged monitor) would hide the overlay.
unsafe fn on_screen(x: i32, y: i32) -> (i32, i32) {
    let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
    let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
    if x >= vx && y >= vy && x < vx + vw - 50 && y < vy + vh - 30 {
        (x, y)
    } else {
        (24, 24)
    }
}

/// `--dump`: one look, written to a text file and a picture next to the settings, for working out what went wrong.
pub fn dump() {
    unsafe {
        let s = Settings::load();
        let t = Templates::builtin();
        let ((gx, gy, gw, gh), has_game) = game_rect();
        let mut log = format!("game window found: {has_game}; client rect on screen: {gx},{gy} {gw}x{gh}\n");
        log += &format!("game window in front: {}\nselected area: {:?}\n", capture::game_in_front(), s.region);
        let (x, y, w, h) = s.region.map_or((gx, gy, gw, gh), |r| (r[0], r[1], r[2], r[3]));
        log += &format!("capture mode: {:?}\n", s.capture);
        // both ways of capturing, whatever the mode, so it is clear which one can see the bar
        for (use_window, name) in [(false, "screen"), (true, "game window")] {
            let (frame, via) = grab(use_window, x, y, w, h);
            log += &format!("--- {name} (got it from: {via})\n");
            if use_window && via == "screen" {
                log += &format!("window capture said: {}\n", capture::window_capture_error());
            }
            match frame {
                None => log += "capture failed\n",
                Some(f) => {
                    log += &format!("captured {}x{}\ndiagnosis: {}\n", f.w, f.h, diagnose(&t, &f).replace('|', " - "));
                    if let Some(dir) = config::dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        save_bmp(&dir.join(if use_window { "dump-window.bmp" } else { "dump-screen.bmp" }), &f);
                    }
                }
            }
        }
        if s.region.is_none() {
            log += "no area selected yet: Settings > Capture > Select area (the whole game window was captured above)\n";
        }
        if let Some(dir) = config::dir() {
            let _ = std::fs::write(dir.join("dump.txt"), &log);
        }
    }
}
