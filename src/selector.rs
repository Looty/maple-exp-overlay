//! `--select-area`: a dimmed full-screen layer to drag a box on, over the EXP text (with the "EXP." label). Runs as its
//! own short process (started from the settings window) and saves the box into the settings file. `--select-map` is
//! the same for the map name in the minimap's header.

use crate::system::wide;
use maple_exp::config::Settings;
use std::cell::RefCell;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The drag in progress: where it started, where the pointer is now, and whether the button is down.
type Drag = ((i32, i32), (i32, i32), bool);

thread_local!(static SEL: RefCell<Drag> = const { RefCell::new(((0, 0), (0, 0), false)) });
// picking the map name's box rather than the EXP text's
thread_local!(static FOR_MAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

pub fn run(for_map: bool) {
    FOR_MAP.with(|m| m.set(for_map));
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide("MapleExpSelector");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_CROSS),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let h = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("select").as_ptr(),
            WS_POPUP | WS_VISIBLE,
            vx,
            vy,
            vw,
            vh,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if h.is_null() {
            return;
        }
        SetLayeredWindowAttributes(h, 0, 120, LWA_ALPHA);
        SetForegroundWindow(h);
        SetFocus(h);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pos = |lp: LPARAM| ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                SEL.with(|s| *s.borrow_mut() = (pos(lp), pos(lp), true));
                0
            }
            WM_MOUSEMOVE => {
                let dragging = SEL.with(|s| {
                    let mut s = s.borrow_mut();
                    if s.2 {
                        s.1 = pos(lp);
                    }
                    s.2
                });
                if dragging {
                    InvalidateRect(hwnd, null(), 1);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let (a, _, dragging) = SEL.with(|s| *s.borrow());
                let b = pos(lp);
                let (l, t, r, bt) = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
                let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
                DestroyWindow(hwnd);
                if dragging && r - l >= 20 && bt - t >= 8 {
                    let mut s = Settings::load();
                    let area = Some([vx + l, vy + t, r - l, bt - t]);
                    if FOR_MAP.with(|m| m.get()) {
                        s.map_region = area;
                    } else {
                        s.region = area;
                    }
                    s.save();
                }
                0
            }
            WM_KEYDOWN => {
                if wp == VK_ESCAPE as usize {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = zeroed();
                windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut r);
                let black = CreateSolidBrush(0);
                FillRect(dc, &r, black);
                DeleteObject(black);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, 0x00ffffff);
                let hint = if FOR_MAP.with(|m| m.get()) {
                    "Drag a box around the map name at the top of the minimap (both lines of text, not the icon). Esc cancels."
                } else {
                    "Drag a box around the EXP text (include the 'EXP.' label). Esc cancels."
                };
                let hint: Vec<u16> = hint.encode_utf16().collect();
                TextOutW(dc, 24, 24, hint.as_ptr(), hint.len() as i32);
                let (a, b, dragging) = SEL.with(|s| *s.borrow());
                if dragging {
                    let sel = RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) };
                    let white = CreateSolidBrush(0x00ffffff);
                    FrameRect(dc, &sel, white);
                    DeleteObject(white);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
