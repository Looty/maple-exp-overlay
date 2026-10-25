//! Getting pixels: the screen (GDI BitBlt) or the game window's own picture (Windows Graphics Capture), and finding
//! the game window.

use crate::wgc::Wgc;
use maple_exp::reader::Frame;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// A screen rectangle from the screen, clipped to the monitors.
pub unsafe fn capture_screen(x: i32, y: i32, cw: i32, ch: i32) -> Option<Frame> {
    let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
    let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
    let (x0, y0) = (x.max(vx), y.max(vy));
    let (x1, y1) = ((x + cw).min(vx + vw), (y + ch).min(vy + vh));
    let (cw, ch) = (x1 - x0, y1 - y0);
    if cw < 8 || ch < 8 {
        return None;
    }
    let screen = GetDC(null_mut());
    let mem = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = cw;
    bi.bmiHeader.biHeight = -ch; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = null_mut();
    let bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    let mut out = None;
    if !bmp.is_null() && !bits.is_null() {
        let old = SelectObject(mem, bmp);
        if BitBlt(mem, 0, 0, cw, ch, screen, x0, y0, SRCCOPY) != 0 {
            let n = (cw * ch * 4) as usize;
            let mut bgra = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
            for px in bgra.chunks_exact_mut(4) {
                px[3] = 255;
            }
            out = Some(Frame { w: cw as usize, h: ch as usize, bgra });
        }
        SelectObject(mem, old);
    }
    if !bmp.is_null() {
        DeleteObject(bmp);
    }
    DeleteDC(mem);
    ReleaseDC(null_mut(), screen);
    out
}

/// Cheap fingerprint, to skip reading a bar that hasn't changed.
pub fn hash(f: &Frame) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in &f.bgra {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// The game's window, if one is found (the largest MapleStory window).
pub unsafe fn game_hwnd() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let best = unsafe { &mut *(lp as *mut (i64, HWND)) };
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return 1;
            }
            let mut cls = [0u16; 128];
            let mut title = [0u16; 256];
            let cl = GetClassNameW(hwnd, cls.as_mut_ptr(), 128) as usize;
            let tl = GetWindowTextW(hwnd, title.as_mut_ptr(), 256) as usize;
            let cls = String::from_utf16_lossy(&cls[..cl]);
            let title = String::from_utf16_lossy(&title[..tl]).to_lowercase();
            if cls == "MapleStoryClass" || title.starts_with("maplestory") {
                let mut r: RECT = zeroed();
                GetClientRect(hwnd, &mut r);
                let area = r.right as i64 * r.bottom as i64;
                if area > best.0 {
                    *best = (area, hwnd);
                }
            }
        }
        1
    }
    let mut best: (i64, HWND) = (0, null_mut());
    if !std::env::args().any(|a| a == "--screen") {
        EnumWindows(Some(each), &mut best as *mut _ as LPARAM);
    }
    best.1
}

/// Whether MapleStory's window exists (a few microseconds: it only walks the list of windows).
pub unsafe fn game_running() -> bool {
    !game_hwnd().is_null()
}

/// The game's client area on screen: (x, y, w, h), and whether a game window was found at all (if not, the primary
/// screen).
pub unsafe fn game_rect() -> ((i32, i32, i32, i32), bool) {
    let hwnd = game_hwnd();
    if !hwnd.is_null() {
        let mut r: RECT = zeroed();
        let mut p = POINT { x: 0, y: 0 };
        GetClientRect(hwnd, &mut r);
        ClientToScreen(hwnd, &mut p);
        if r.right >= 100 && r.bottom >= 100 {
            return ((p.x, p.y, r.right, r.bottom), true);
        }
    }
    ((0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)), false)
}

thread_local!(static WGC: RefCell<Option<Wgc>> = const { RefCell::new(None) });
thread_local!(static WGC_WAIT: Cell<u32> = const { Cell::new(0) });
thread_local!(static WGC_ERR: RefCell<String> = const { RefCell::new(String::new()) });

/// Why the last window capture gave nothing, in words.
pub fn window_capture_error() -> String {
    WGC_ERR.with(|m| m.borrow().clone())
}

/// The same screen rectangle as `capture_screen`, but from the game window's own picture (Windows Graphics
/// Capture), so a taskbar or another window on top of the game isn't in it. None if that isn't possible.
pub unsafe fn capture_window(hwnd: HWND, x: i32, y: i32, cw: i32, ch: i32) -> Option<Frame> {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        WGC.with(|g| *g.borrow_mut() = None);
        return None;
    }
    let mut wr: RECT = zeroed();
    GetWindowRect(hwnd, &mut wr);
    let (ox, oy) = (x - wr.left, y - wr.top); // the wanted box, in window coordinates
    WGC.with(|g| {
        let mut g = g.borrow_mut();
        if g.as_ref().is_none_or(|s| s.hwnd() != hwnd as isize) {
            *g = None;
            if WGC_WAIT.with(|c| c.get()) > 0 {
                WGC_WAIT.with(|c| c.set(c.get() - 1));
                return None;
            }
            match Wgc::new(hwnd as isize) {
                Ok(s) => *g = Some(s),
                Err(e) => {
                    WGC_ERR.with(|m| *m.borrow_mut() = format!("{e}"));
                    WGC_WAIT.with(|c| c.set(30)); // don't retry (and pay for it) every look
                    return None;
                }
            }
        }
        let f = g.as_mut().and_then(|s| s.grab(ox, oy, cw, ch));
        if f.is_none() {
            WGC_ERR.with(|m| *m.borrow_mut() = "no frame, or the area is outside the window".to_string());
        }
        f
    })
    // (a blank picture counts as nothing: the caller falls back to the screen)
    .filter(|f| f.bgra.chunks_exact(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0))
}

/// Looks at a screen rectangle: through the game window's own picture when allowed, else, or if Windows gives
/// nothing, from the screen. Second value: which one it was.
pub unsafe fn grab(use_window: bool, x: i32, y: i32, w: i32, h: i32) -> (Option<Frame>, &'static str) {
    if use_window {
        if let Some(f) = capture_window(game_hwnd(), x, y, w, h) {
            return (Some(f), "game window");
        }
    }
    (capture_screen(x, y, w, h), "screen")
}

/// Whether the game is the window in front (a debugging aid: the taskbar behaves differently then).
pub unsafe fn game_in_front() -> bool {
    let g = game_hwnd();
    !g.is_null() && GetForegroundWindow() == g
}

/// Saves a frame as a 32-bit BMP.
pub fn save_bmp(path: &std::path::Path, f: &Frame) {
    let size = 54 + f.bgra.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(f.w as i32).to_le_bytes());
    out.extend_from_slice(&(-(f.h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    out.extend_from_slice(&f.bgra);
    let _ = std::fs::write(path, out);
}
