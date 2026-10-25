//! Windows integration: start with Windows, and one running copy of each part (overlay, settings window).

use maple_exp::config::APP_NAME;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::CreateMutexW;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        let found = RegQueryValueExW(key, wide(APP_NAME).as_ptr(), null(), null_mut(), null_mut(), null_mut()) == 0;
        RegCloseKey(key);
        found
    }
}

/// Adds or removes the Run entry, so Windows starts the overlay at sign-in.
pub fn set_autostart(on: bool) {
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, KEY_READ | KEY_WRITE, &mut key) != 0 {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let value = wide(&format!("\"{}\"", exe.display()));
                RegSetValueExW(key, wide(APP_NAME).as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32);
            }
        } else {
            RegDeleteValueW(key, wide(APP_NAME).as_ptr());
        }
        RegCloseKey(key);
    }
}

/// Keeps the registered path current if the exe was moved or rebuilt elsewhere.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

/// True if this is the first copy to claim `name`. The handle is never closed: it lives as long as the process.
pub fn claim(name: &str) -> bool {
    unsafe {
        let h = CreateMutexW(null(), 1, wide(name).as_ptr());
        let already = windows_sys::Win32::Foundation::GetLastError() == ERROR_ALREADY_EXISTS;
        !(h.is_null() || already)
    }
}
