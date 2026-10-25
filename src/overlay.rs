//! The overlay: one small always-on-top window, one timer. Plain Win32 + GDI, no UI framework, so it costs next to
//! nothing. Everything about how it looks and what it shows comes from the settings file, which it re-reads
//! whenever the settings window saves.

use crate::capture::{self, game_rect, grab, hash, save_bmp};
use crate::system::{self, wide};
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
const SLOW_MS: u32 = 3000; // while no area is selected: just waiting for one
const IDLE_MS: u32 = 5000; // while MapleStory isn't running
const LOST_AFTER: u32 = 5; // failed looks in a row before saying the bar can't be read
const WM_TRAY: u32 = WM_APP + 1;
pub const WM_SHOW: u32 = WM_APP + 2; // another launch of the exe asks this one to open the settings
const ID_SETTINGS: usize = 1;
const ID_RESET: usize = 2;
const ID_TOGGLE: usize = 3;
const ID_QUIT: usize = 4;
pub const CLASS: &str = "MapleExpOverlay";

/// A text colour as an item's tone number (see `config::tone_hex`).
type Tone = u8;
const TONE_STATUS: Tone = 2;

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
    } else {
        format!("{:.0} min", min.max(1.0))
    }
}

fn clock(secs: f64) -> String {
    let t = secs as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

type Row = Vec<(String, Tone)>;

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
        "eta" => {
            let m = if s.eta_recent { st.eta_recent_min.or(st.eta_min) } else { st.eta_min };
            m.map(|m| format!("Lv in {}", eta(m)))
        }
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
    let mut rows: Vec<Row> = Vec::new();
    for it in s.items.iter().filter(|i| i.show) {
        let Some(text) = item_text(&it.id, s, st, problem.is_some()) else { continue };
        if it.newline || rows.is_empty() {
            rows.push(Vec::new());
        }
        rows.last_mut().unwrap().push((text, it.tone));
    }
    if let Some(p) = problem {
        rows.push(vec![(p, TONE_STATUS)]);
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
        }
        let (frame, via) = grab(self.s.capture == CaptureMode::Window, x, y, cw, ch);
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
                    let templates = self.templates.get_or_insert_with(Templates::builtin);
                    let r = find_and_read(templates, &f).map(|(_, r)| r);
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
    }

    unsafe fn set_look_timer(&self) {
        // no area selected: nothing to look at, only wait for the box being set; no game: look rarely
        let ms = if self.s.region.is_none() {
            SLOW_MS
        } else if !self.game_up {
            IDLE_MS
        } else {
            self.s.poll_secs as u32 * 1000
        };
        SetTimer(self.hwnd, TIMER_LOOK, ms, None);
    }

    // ------------------------------------------------------------ look and feel

    unsafe fn make_font(&mut self) {
        if !self.font.is_null() {
            DeleteObject(self.font);
        }
        let height = -(self.s.font_size * self.dpi as i32 / 72);
        let weight = if self.s.bold { 700 } else { 400 };
        self.font = CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(&self.s.font).as_ptr());
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
        let st = self.tracker.stats(now);
        // hide by itself when nothing has come in for a while, and come back with the next EXP
        let idle_for = st.since_gain.unwrap_or(now);
        let want_hidden = self.s.hide_idle && idle_for >= self.s.hide_after_secs as f64;
        if want_hidden != self.auto_hidden {
            self.auto_hidden = want_hidden;
            self.sync_visibility();
        }
        let short_note = self.note.split('|').next().unwrap_or("");
        let status = if !self.status.is_empty() && !short_note.is_empty() { short_note } else { self.status };
        let rows = build_rows(&self.s, &st, status);
        if rows == self.rows {
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
        if size != self.size {
            self.size = size;
            SetWindowPos(self.hwnd, null_mut(), 0, 0, size.0, size.1, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
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
            for (text, tone) in row {
                SetTextColor(mem, rgb(config::tone_hex(&self.s, *tone)));
                let t: Vec<u16> = text.encode_utf16().collect();
                TextOutW(mem, x, pad - self.dpi as i32 / 96 * 2 + i as i32 * line, t.as_ptr(), t.len() as i32);
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
    s.pos = Some([r.left, r.top]);
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
                    } else if wp == TIMER_CFG {
                        app.check_config();
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
        SetTimer(hwnd, TIMER_LOOK, 500, None);
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
