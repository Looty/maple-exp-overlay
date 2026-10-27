//! `--settings`: the settings window (Slint, software renderer). It is its own short-lived process: it edits the
//! settings file, and the running overlay notices the change within a second and applies it. Closing the window
//! ends the process, so none of this stays in memory while you play.

use crate::capture::grab;
use crate::system::{self, wide};
use maple_exp::config::{self, parse_hex, CaptureMode, Settings, LAYOUTS, THEMES};
use maple_exp::editor::{self, Geo};
use maple_exp::reader::{diagnose, find_and_read, Reading, Templates};
use maple_exp::text::commas;
use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

slint::include_modules!();

const POLL_MS: [i32; 4] = [250, 500, 750, 1000];
const TITLE: &str = "Maple EXP Overlay - Settings";

type Shared = Rc<RefCell<Settings>>;

fn color(hex: &str) -> slint::Color {
    let (r, g, b) = parse_hex(hex).unwrap_or((255, 255, 255));
    slint::Color::from_rgb_u8(r, g, b)
}

fn region_text(s: &Settings) -> String {
    match s.region {
        Some([x, y, w, h]) => format!("{w} x {h} at {x}, {y}"),
        None => "Not selected yet".to_string(),
    }
}

/// Changes the settings and saves. The file is read again first: the overlay saves its position when it is dragged and
/// the area picker saves the box, and neither should be overwritten with this window's older copy.
fn edit(settings: &Shared, f: impl FnOnce(&mut Settings)) {
    let mut s = settings.borrow_mut();
    *s = Settings::load();
    f(&mut s);
    s.clamp();
    s.save();
}

fn to_cfg(s: &Settings) -> Cfg {
    let [px, py] = s.pos.unwrap_or([24, 24]);
    Cfg {
        font_size: s.font_size,
        bold: s.bold,
        opacity: s.opacity,
        bg: color(&s.bg),
        accent: color(&s.accent),
        text_color: color(&s.text),
        dim: color(&s.dim),
        bg_hex: s.bg.clone().into(),
        accent_hex: s.accent.clone().into(),
        text_hex: s.text.clone().into(),
        dim_hex: s.dim.clone().into(),
        locked: s.locked,
        topmost: s.topmost,
        taskbar: s.taskbar,
        tray: s.tray,
        pos_x: px,
        pos_y: py,
        hide_idle: s.hide_idle,
        hide_after: s.hide_after_secs,
        new_session_idle: s.new_session_idle,
        new_session_secs: s.new_session_secs,
        short_numbers: s.short_numbers,
        eta_recent: s.eta_recent,
        idle_secs: s.idle_secs,
        window_min: s.window_min,
        capture_index: match s.capture {
            CaptureMode::Screen => 0,
            CaptureMode::Window => 1,
        },
        // (a hand-edited value in between shows as the nearest choice)
        poll_index: (0..POLL_MS.len()).min_by_key(|&i| (POLL_MS[i] - s.poll_ms).abs()).unwrap_or(0) as i32,
        anim_count: s.anim_count,
        anim_pop: s.anim_pop,
        anim_snail: s.anim_snail,
        anim_rainbow: s.anim_rainbow,
        autostart: system::autostart_enabled(),
        start_hidden: s.start_hidden,
    }
}

/// The window's values back into the settings (colours and font are set by their own callbacks).
fn from_cfg(s: &mut Settings, c: &Cfg) {
    s.font_size = c.font_size;
    s.bold = c.bold;
    s.opacity = c.opacity;
    s.locked = c.locked;
    s.topmost = c.topmost;
    s.taskbar = c.taskbar;
    s.tray = c.tray;
    s.hide_idle = c.hide_idle;
    s.hide_after_secs = c.hide_after;
    s.new_session_idle = c.new_session_idle;
    s.new_session_secs = c.new_session_secs;
    s.short_numbers = c.short_numbers;
    s.eta_recent = c.eta_recent;
    s.idle_secs = c.idle_secs;
    s.window_min = c.window_min;
    s.capture = if c.capture_index == 1 { CaptureMode::Window } else { CaptureMode::Screen };
    s.poll_ms = POLL_MS[(c.poll_index.max(0) as usize).min(POLL_MS.len() - 1)];
    s.anim_count = c.anim_count;
    s.anim_pop = c.anim_pop;
    s.anim_snail = c.anim_snail;
    s.anim_rainbow = c.anim_rainbow;
    s.start_hidden = c.start_hidden;
}

fn installed_fonts() -> Vec<String> {
    unsafe extern "system" fn each(lf: *const LOGFONTW, _tm: *const TEXTMETRICW, _kind: u32, lp: isize) -> i32 {
        let names = unsafe { &mut *(lp as *mut Vec<String>) };
        let face = unsafe { &(*lf).lfFaceName };
        let len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let name = String::from_utf16_lossy(&face[..len]);
        if !name.starts_with('@') && !names.contains(&name) {
            names.push(name);
        }
        1
    }
    let mut names: Vec<String> = Vec::new();
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfCharSet = 1; // DEFAULT_CHARSET
        EnumFontFamiliesExW(dc, &lf, Some(each), &mut names as *mut _ as isize, 0);
        ReleaseDC(std::ptr::null_mut(), dc);
    }
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// What a drag in the layout editor is doing right now.
#[derive(Default)]
struct Drag {
    /// The item whose chip was pressed.
    item: Option<usize>,
    start: (f32, f32),
    /// Where inside the chip it was grabbed.
    grab: (f32, f32),
    /// Moved far enough to be a drag rather than a click.
    moved: bool,
    target: Option<editor::Target>,
}

fn tone_color(s: &Settings, tone: u8) -> slint::Color {
    color(config::tone_hex(s, tone))
}

/// The colours offered when right-clicking a chip: the three theme colours, then the fixed palette. A colour's position
/// here is its tone number.
fn palette(s: &Settings) -> Vec<slint::Color> {
    (0..config::TONES).map(|t| tone_color(s, t)).collect()
}

/// The chips (and the editor's size) for the current items, font and colours.
fn rebuild_chips(ui: &SettingsWindow, s: &Settings) {
    let g = Geo::new(s.font_size);
    let (placed, height) = editor::place(&s.items, &g);
    let chips: Vec<Chip> = placed
        .iter()
        .map(|p| {
            let it = &s.items[p.idx];
            Chip { label: editor::sample(&it.id).into(), x: p.x, y: p.y, w: p.w, h: p.h, color: tone_color(s, it.tone), shown: it.show }
        })
        .collect();
    ui.set_chips(ModelRc::new(VecModel::from(chips)));
    ui.set_palette(ModelRc::new(VecModel::from(palette(s))));
    ui.set_editor_h(height);
    ui.set_chip_font(g.font_px);
    ui.set_dragging(false);
}

/// Said under a failed test taken while the game isn't in front.
const NOT_IN_FRONT: &str = "Note: the game wasn't in front for this test. With the game in front, Windows normally hides the taskbar over a borderless fullscreen game, so a taskbar covering the bar here may not happen while you play. Use \"Test in game\" and switch to the game.";

/// What the Test button found.
struct Outcome {
    text: String,
    ok: bool,
    /// What the chosen capture method sees in the box.
    image: Option<slint::Image>,
    caption: String,
    /// The other capture method, when it can read the bar and the chosen one can't (offered as a button; never applied
    /// by itself).
    switch_to: Option<CaptureMode>,
}

fn mode_name(m: CaptureMode) -> &'static str {
    match m {
        CaptureMode::Screen => "Screen",
        CaptureMode::Window => "Game window",
    }
}

fn reading_text(r: &Reading) -> String {
    match r.exp {
        Some(e) => format!("EXP {} [{:.2}%]", commas(e), r.pct),
        None => format!("[{:.2}%] (the game shows only the percentage)", r.pct),
    }
}

/// The short reason from `diagnose`, in plain words, with what to do about it.
fn explain(short: &str) -> &'static str {
    if short.starts_with("capture is black") {
        "The capture is black. The game is probably in fullscreen-exclusive mode: switch it to borderless or windowed fullscreen."
    } else if short.starts_with("no [ ]") {
        "The EXP text isn't in the box. Select the area again."
    } else if short.starts_with("text cut off") {
        "Part of the EXP text is cut off or covered, often by the Windows taskbar or another window. Select a bigger box, or capture the game window instead."
    } else if short.starts_with("digits unclear") {
        "The EXP text is in the box but the digits aren't clear. Select the area again with a little more margin."
    } else {
        "The EXP bar couldn't be read."
    }
}

/// One test of the selected area with one way of capturing: what was read (or why not), and the picture of what was seen.
fn test_capture(region: [i32; 4], t: &Templates, use_window: bool) -> (Result<Reading, &'static str>, Option<slint::Image>) {
    let [x, y, w, h] = region;
    match unsafe { grab(use_window, x, y, w, h) } {
        (None, _) => (Err("The capture failed."), None),
        (Some(f), _) => {
            let mut px = SharedPixelBuffer::<slint::Rgba8Pixel>::new(f.w as u32, f.h as u32);
            for (dst, src) in px.make_mut_slice().iter_mut().zip(f.bgra.chunks_exact(4)) {
                *dst = slint::Rgba8Pixel { r: src[2], g: src[1], b: src[0], a: 255 };
            }
            let image = Some(slint::Image::from_rgba8(px));
            match find_and_read(t, &f) {
                Some((_, r)) => (Ok(r), image),
                None => (Err(explain(diagnose(t, &f).split('|').next().unwrap_or(""))), image),
            }
        }
    }
}

/// The Test button: tries the chosen capture method and says plainly what to do next.
fn run_test(s: &Settings) -> Outcome {
    let Some(region) = s.region else {
        return Outcome {
            text: "No area selected yet. Click \"Select area...\" and drag a box around the EXP text in the game.".into(),
            ok: false,
            image: None,
            caption: String::new(),
            switch_to: None,
        };
    };
    let t = Templates::builtin();
    let in_front = unsafe { crate::capture::game_in_front() };
    let (chosen, other) = match s.capture {
        CaptureMode::Window => (CaptureMode::Window, CaptureMode::Screen),
        CaptureMode::Screen => (CaptureMode::Screen, CaptureMode::Window),
    };
    let (first, image) = test_capture(region, &t, chosen == CaptureMode::Window);
    let caption = format!("What the \"{}\" capture sees in your box:", mode_name(chosen));
    match first {
        Ok(r) => Outcome {
            text: format!("Works. Read {} with the \"{}\" capture.", reading_text(&r), mode_name(chosen)),
            ok: true,
            image,
            caption,
            switch_to: None,
        },
        Err(why) => {
            let (second, _) = test_capture(region, &t, other == CaptureMode::Window);
            let (tail, switch_to) = match second {
                Ok(r) => (format!("The \"{}\" capture can read it ({}).", mode_name(other), reading_text(&r)), Some(other)),
                Err(_) => (format!("The \"{}\" capture can't read it either.", mode_name(other)), None),
            };
            let note = if in_front { "" } else { NOT_IN_FRONT };
            Outcome {
                text: format!("Can't read the EXP bar with the \"{}\" capture.\n{why}\n{tail}\n{note}", mode_name(chosen)),
                ok: false,
                image,
                caption,
                switch_to,
            }
        }
    }
}

fn font_index(fonts: &[String], name: &str) -> i32 {
    fonts.iter().position(|f| f.eq_ignore_ascii_case(name)).map_or(0, |i| i as i32)
}

/// Brings this process's own settings window to the front (also over a borderless-fullscreen game).
fn focus_own() {
    unsafe {
        let h = FindWindowW(std::ptr::null(), wide(TITLE).as_ptr());
        if !h.is_null() {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
    }
}

/// Another settings process is already running. If its window answers, bring it forward and return true (this launch
/// then has nothing to do). If it is stuck (a blank window that never comes back), or has no window at all, end it
/// and return false: this launch opens a fresh window instead, so launching again is never a dead end.
fn take_over_existing() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    unsafe {
        // (the other copy may still be starting up: give its window a moment to appear)
        let mut h = std::ptr::null_mut();
        for _ in 0..10 {
            h = FindWindowW(std::ptr::null(), wide(TITLE).as_ptr());
            if !h.is_null() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if h.is_null() {
            return false; // no window to bring forward: open one (a rare second window beats nothing opening)
        }
        // never call into a hung window directly (that would hang this process too): ask with a time limit first
        let mut res = 0usize;
        let answers = SendMessageTimeoutW(h, WM_NULL, 0, 0, SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, &mut res) != 0;
        if answers {
            ShowWindowAsync(h, SW_RESTORE);
            SetForegroundWindow(h);
            RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN);
            return true;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if pid != 0 && pid != std::process::id() {
            let p = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
            if !p.is_null() {
                TerminateProcess(p, 1);
                WaitForSingleObject(p, 2000); // gone before our window opens, so nothing finds the old one by its title
                CloseHandle(p);
            }
        }
        false
    }
}

pub fn run() {
    // (when the claim fails and the old copy is stuck, it is ended and this one carries on: the mutex handle this
    // process now holds keeps the name taken for the next launch)
    if !system::claim("Local\\MapleExpOverlay.Settings") && take_over_existing() {
        return;
    }
    let ui = SettingsWindow::new().expect("settings window");
    let settings: Shared = Rc::new(RefCell::new(Settings::load()));
    let fonts = installed_fonts();
    let last_pos = Rc::new(Cell::new((0, 0)));

    // everything in the window, from the settings
    let refresh = {
        let ui = ui.as_weak();
        let settings = settings.clone();
        let fonts = fonts.clone();
        let last_pos = last_pos.clone();
        move || {
            let ui = ui.unwrap();
            let s = settings.borrow();
            let cfg = to_cfg(&s);
            last_pos.set((cfg.pos_x, cfg.pos_y));
            ui.set_cfg(cfg);
            ui.set_font_index(font_index(&fonts, &s.font));
            ui.set_font_name(s.font.clone().into());
            ui.set_region_text(region_text(&s).into());
            rebuild_chips(&ui, &s);
        }
    };

    ui.set_fonts(ModelRc::new(VecModel::from(fonts.iter().map(|f| SharedString::from(f.as_str())).collect::<Vec<_>>())));
    ui.set_themes(ModelRc::new(VecModel::from(THEMES.iter().map(|t| SharedString::from(t.0)).collect::<Vec<_>>())));
    ui.set_layouts(ModelRc::new(VecModel::from(LAYOUTS.iter().map(|l| SharedString::from(l.0)).collect::<Vec<_>>())));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    // nothing is read until an area is selected, so a first run opens on the tab where that is done
    if settings.borrow().region.is_none() {
        ui.set_start_tab(2);
    }
    ui.set_data_folder(config::dir().map(|d| d.display().to_string()).unwrap_or_default().into());
    refresh();

    // any change in the window is saved at once; the overlay picks it up on its next tick
    ui.on_changed({
        let ui = ui.as_weak();
        let settings = settings.clone();
        let last_pos = last_pos.clone();
        move || {
            let cfg = ui.unwrap().get_cfg();
            let moved = (cfg.pos_x, cfg.pos_y) != last_pos.get();
            last_pos.set((cfg.pos_x, cfg.pos_y));
            edit(&settings, |s| {
                from_cfg(s, &cfg);
                if moved {
                    s.pos = Some([cfg.pos_x, cfg.pos_y]);
                }
            });
            if cfg.autostart != system::autostart_enabled() {
                system::set_autostart(cfg.autostart);
            }
            rebuild_chips(&ui.unwrap(), &settings.borrow()); // (the font size changes the chips)
        }
    });

    ui.on_theme_picked({
        let settings = settings.clone();
        let refresh = refresh.clone();
        move |i| {
            if let Some(t) = THEMES.get(i as usize) {
                edit(&settings, |s| (s.bg, s.accent, s.text, s.dim) = (t.1.into(), t.2.into(), t.3.into(), t.4.into()));
                refresh();
            }
        }
    });

    ui.on_font_picked({
        let ui = ui.as_weak();
        let settings = settings.clone();
        move |name| {
            edit(&settings, |s| s.font = name.to_string());
            ui.unwrap().set_font_name(name);
        }
    });

    ui.on_hex_edited({
        let ui = ui.as_weak();
        let settings = settings.clone();
        move |which, text| {
            let Some((r, g, b)) = parse_hex(&text) else { return }; // (half-typed: wait until it's a colour)
            let hex = format!("#{r:02x}{g:02x}{b:02x}");
            let ui = ui.unwrap();
            let mut cfg = ui.get_cfg();
            let c = slint::Color::from_rgb_u8(r, g, b);
            match which {
                0 => cfg.bg = c,
                1 => cfg.accent = c,
                2 => cfg.text_color = c,
                _ => cfg.dim = c,
            }
            edit(&settings, |s| match which {
                0 => s.bg = hex,
                1 => s.accent = hex,
                2 => s.text = hex,
                _ => s.dim = hex,
            });
            ui.set_cfg(cfg);
            rebuild_chips(&ui, &settings.borrow());
        }
    });

    ui.on_layout_picked({
        let settings = settings.clone();
        let ui = ui.as_weak();
        move |i| {
            if let Some(l) = LAYOUTS.get(i as usize) {
                edit(&settings, |s| s.items = (l.1)());
                rebuild_chips(&ui.unwrap(), &settings.borrow());
            }
        }
    });

    // ---- the layout editor: press a chip and drag it; a press without a drag shows or hides it; right-click recolours
    let drag = Rc::new(RefCell::new(Drag::default()));
    let chip_at = |s: &Settings, x: f32, y: f32| {
        let (placed, _) = editor::place(&s.items, &Geo::new(s.font_size));
        let idx = editor::hit(&placed, x, y)?;
        let p = *placed.iter().find(|p| p.idx == idx)?;
        Some((idx, p))
    };

    ui.on_drag_down({
        let drag = drag.clone();
        let settings = settings.clone();
        move |x, y| {
            let s = settings.borrow();
            let mut d = drag.borrow_mut();
            *d = Drag::default();
            if let Some((idx, p)) = chip_at(&s, x, y) {
                d.item = Some(idx);
                d.start = (x, y);
                d.grab = (x - p.x, y - p.y);
            }
        }
    });

    ui.on_drag_move({
        let drag = drag.clone();
        let settings = settings.clone();
        let ui = ui.as_weak();
        move |x, y| {
            let mut d = drag.borrow_mut();
            let Some(idx) = d.item else { return };
            if !d.moved && (x - d.start.0).abs() + (y - d.start.1).abs() < 6.0 {
                return;
            }
            d.moved = true;
            let s = settings.borrow();
            let g = Geo::new(s.font_size);
            let (placed, _) = editor::place(&s.items, &g);
            let Some(p) = placed.iter().find(|p| p.idx == idx) else { return };
            let it = &s.items[idx];
            let t = editor::target(&s.items, &placed, &g, idx, x, y);
            d.target = Some(t);
            let ui = ui.unwrap();
            ui.set_ghost(Chip {
                label: editor::sample(&it.id).into(),
                x: x - d.grab.0,
                y: y - d.grab.1,
                w: p.w,
                h: p.h,
                color: tone_color(&s, it.tone),
                shown: true,
            });
            ui.set_caret_x(t.caret.0);
            ui.set_caret_y(t.caret.1);
            ui.set_caret_h(t.caret.2);
            ui.set_newrow_y(g.pad + editor::rows(&s.items).len() as f32 * (g.chip_h + g.gap) + g.chip_h * 0.25);
            ui.set_dragging(true);
        }
    });

    ui.on_drag_up({
        let drag = drag.clone();
        let settings = settings.clone();
        let ui = ui.as_weak();
        move |_x, _y| {
            let (item, moved, target) = {
                let mut d = drag.borrow_mut();
                let out = (d.item, d.moved, d.target);
                *d = Drag::default();
                out
            };
            let Some(idx) = item else { return };
            edit(&settings, |s| {
                if !moved {
                    if let Some(it) = s.items.get_mut(idx) {
                        it.show = !it.show;
                    }
                } else if let Some(t) = target {
                    editor::apply_move(&mut s.items, idx, t.row, t.col);
                }
            });
            rebuild_chips(&ui.unwrap(), &settings.borrow());
        }
    });

    // right-click a chip: remember which one and let the window open its colour palette (returns whether a chip was hit)
    ui.on_chip_right_click({
        let settings = settings.clone();
        let ui = ui.as_weak();
        move |x, y| match chip_at(&settings.borrow(), x, y) {
            Some((idx, _)) => {
                ui.unwrap().set_color_target(idx as i32);
                true
            }
            None => false,
        }
    });

    ui.on_color_picked({
        let settings = settings.clone();
        let ui = ui.as_weak();
        move |tone| {
            let ui = ui.unwrap();
            let target = ui.get_color_target();
            if target >= 0 {
                edit(&settings, |s| {
                    if let Some(it) = s.items.get_mut(target as usize) {
                        it.tone = (tone.max(0) as u8).min(config::TONES - 1);
                    }
                });
                rebuild_chips(&ui, &settings.borrow());
            }
        }
    });

    ui.on_select_area(|| {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--select-area").spawn();
        }
    });

    // Test: reads the selected box with the chosen capture method and shows the result
    let show_test = Rc::new({
        let ui = ui.as_weak();
        move || {
            let ui = ui.unwrap();
            let o = run_test(&Settings::load());
            ui.set_test_text(o.text.into());
            ui.set_test_ok(o.ok);
            ui.set_test_caption(o.caption.into());
            ui.set_has_test_image(o.image.is_some());
            if let Some(img) = o.image {
                ui.set_test_image(img);
            }
            ui.set_switch_label(
                o.switch_to.map_or(String::new(), |m| format!("Switch to \"{}\" capture and test again", mode_name(m))).into(),
            );
        }
    });
    ui.on_test_area({
        let show_test = show_test.clone();
        move || show_test()
    });
    // "Test in game": a few seconds to switch to the game, then the same test, taken while the game is in front
    let countdown = Rc::new(Timer::default());
    ui.on_test_in_game({
        let ui = ui.as_weak();
        let show_test = show_test.clone();
        let timer = countdown.clone();
        move || {
            let left = Rc::new(Cell::new(5));
            ui.unwrap().set_test_countdown(5);
            let (ui, show_test, stop) = (ui.clone(), show_test.clone(), timer.clone());
            timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
                left.set(left.get() - 1);
                ui.unwrap().set_test_countdown(left.get().max(0));
                if left.get() <= 0 {
                    stop.stop();
                    show_test();
                }
            });
        }
    });
    // the button the test offers when the other capture method can read the bar: the user's click, nothing automatic
    ui.on_switch_capture({
        let settings = settings.clone();
        let refresh = refresh.clone();
        let show_test = show_test.clone();
        move || {
            edit(&settings, |s| {
                s.capture = if s.capture == CaptureMode::Window { CaptureMode::Screen } else { CaptureMode::Window };
            });
            refresh();
            show_test();
        }
    });

    ui.on_reset_position({
        let settings = settings.clone();
        let refresh = refresh.clone();
        move || {
            edit(&settings, |s| s.pos = None);
            refresh();
        }
    });

    ui.on_reset_all({
        let settings = settings.clone();
        let refresh = refresh.clone();
        move || {
            system::set_autostart(false);
            edit(&settings, |s| *s = Settings::default());
            refresh();
        }
    });

    // updates: the check and the download run on their own thread, so the window keeps drawing meanwhile
    let found = std::sync::Arc::new(std::sync::Mutex::new(None::<maple_exp::update::Release>));
    ui.on_check_update({
        let ui = ui.as_weak();
        let found = found.clone();
        move || {
            let ui = ui.unwrap();
            ui.set_update_busy(true);
            ui.set_update_tag("".into());
            ui.set_update_text("Checking...".into());
            let (weak, found) = (ui.as_weak(), found.clone());
            std::thread::spawn(move || {
                let result = crate::updater::check();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_update_busy(false);
                    match result {
                        Ok(Some(rel)) => {
                            ui.set_update_text(format!("Version {} is available.", rel.tag).into());
                            ui.set_update_tag(rel.tag.clone().into());
                            *found.lock().unwrap() = Some(rel);
                        }
                        Ok(None) => ui.set_update_text(format!("You have the latest version ({}).", env!("CARGO_PKG_VERSION")).into()),
                        Err(e) => ui.set_update_text(format!("Couldn't check for updates: {e}").into()),
                    }
                });
            });
        }
    });
    ui.on_install_update({
        let ui = ui.as_weak();
        let found = found.clone();
        move || {
            let Some(rel) = found.lock().unwrap().clone() else { return };
            let ui = ui.unwrap();
            ui.set_update_busy(true);
            ui.set_update_text(format!("Downloading {}...", rel.tag).into());
            let weak = ui.as_weak();
            std::thread::spawn(move || {
                let result = crate::updater::install(&rel);
                if let Ok(exe) = &result {
                    crate::updater::restart_overlay(exe);
                }
                let _ = weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(_) => {
                        ui.set_update_tag("".into());
                        ui.set_update_text(
                            format!("Updated to {}. The overlay restarted with it; this window closes now.", rel.tag).into(),
                        );
                        // (this window is still the old version: close it so the next one opened is the new one)
                        slint::Timer::single_shot(Duration::from_millis(2500), || {
                            let _ = slint::quit_event_loop();
                        });
                    }
                    Err(e) => {
                        ui.set_update_busy(false);
                        ui.set_update_text(format!("The update wasn't installed: {e}").into());
                    }
                });
            });
        }
    });

    ui.on_open_folder(|| {
        if let Some(d) = config::dir() {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::process::Command::new("explorer").arg(d).spawn();
        }
    });

    // the area picker runs as another process and writes the file: show its result when it lands
    let seen = Cell::new(config::modified());
    let watch = Timer::default();
    watch.start(TimerMode::Repeated, Duration::from_millis(600), {
        let ui = ui.as_weak();
        move || {
            let m = config::modified();
            if m != seen.get() {
                seen.set(m);
                ui.unwrap().set_region_text(region_text(&Settings::load()).into());
            }
        }
    });

    // come to the front even over a borderless-fullscreen game (a freshly started process isn't allowed to by default;
    // the overlay grants that when it starts us)
    ui.show().ok();
    let front = Timer::default();
    front.start(TimerMode::SingleShot, Duration::from_millis(150), focus_own);
    ui.run().ok();
}
