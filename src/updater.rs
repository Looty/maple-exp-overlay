//! In-app updates, the Windows part: asks GitHub for the latest release, downloads the exe and its signature over
//! Windows' own HTTPS (WinHTTP, so no TLS library ships in the exe), checks the signature (`maple_exp::update`), swaps
//! the exe and restarts the overlay. Used only by the settings window, so the overlay never loads any of it.
//!
//! The swap: Windows won't overwrite or delete a running exe, but it will rename one. The running exe becomes
//! `maple-exp-overlay.old`, the new one takes its name, and the overlay deletes the `.old` file when it next starts.

use crate::system::wide;
use maple_exp::update::{self, Release};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

const MAX_DOWNLOAD: usize = 64 << 20; // far more than the exe; stops a runaway answer filling memory

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// GET an https URL; the body, or what went wrong in words. Redirects (GitHub's downloads use them) are followed.
pub fn http_get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only https downloads are allowed")?;
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let fail = |what: &str| format!("{what} failed (network error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
    unsafe {
        let agent = wide(&format!("maple-exp-overlay/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(fail("starting the download"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let conn = Handle(WinHttpConnect(session.0, wide(host).as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(fail("connecting"));
        }
        let req =
            Handle(WinHttpOpenRequest(conn.0, wide("GET").as_ptr(), wide(&path).as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE));
        if req.0.is_null() {
            return Err(fail("the request"));
        }
        let headers = wide(&format!("Accept: {accept}\r\n"));
        if WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(req.0, null_mut()) == 0 {
            return Err(fail("the connection to GitHub"));
        }
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            null_mut(),
        );
        if status != 200 {
            return Err(match status {
                404 => "nothing found on GitHub (no release yet?)".to_string(),
                403 | 429 => "GitHub says too many checks from this network: try again in an hour".to_string(),
                _ => format!("GitHub answered with error {status}"),
            });
        }
        let mut body = Vec::new();
        loop {
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(req.0, &mut avail) == 0 {
                return Err(fail("the download"));
            }
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(req.0, body[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                return Err(fail("the download"));
            }
            body.truncate(start + read as usize);
            if body.len() > MAX_DOWNLOAD {
                return Err("the download is far too big: stopped".to_string());
            }
        }
        Ok(body)
    }
}

/// The latest release, if it is newer than this version and installable; None when this is already the latest.
pub fn check() -> Result<Option<Release>, String> {
    if update::PUBLIC_KEY.is_empty() {
        return Err("Updates aren't set up in this build.".to_string());
    }
    let url = format!("https://api.github.com/repos/{}/releases/latest", update::REPO);
    let json = http_get(&url, "application/vnd.github+json")?;
    let rel = update::parse_latest(&json)?;
    Ok(update::is_newer(&rel.tag, env!("CARGO_PKG_VERSION")).then_some(rel))
}

/// Downloads `rel`, checks its signature, and puts it in place of the running exe. Returns the exe's path.
pub fn install(rel: &Release) -> Result<PathBuf, String> {
    let exe = http_get(&rel.exe_url, "application/octet-stream")?;
    let sig = http_get(&rel.sig_url, "application/octet-stream")?;
    update::verify(&exe, &String::from_utf8_lossy(&sig), update::PUBLIC_KEY, &rel.tag)?;

    let cur = std::env::current_exe().map_err(|e| format!("can't find this program's file: {e}"))?;
    let (new, old) = (cur.with_extension("new"), cur.with_extension("old"));
    let denied = |e: std::io::Error| format!("can't write next to the program ({e}); move it to a folder you own, or update by hand");
    std::fs::write(&new, &exe).map_err(denied)?;
    let _ = std::fs::remove_file(&old); // (left from the last update, if the overlay hasn't cleared it yet)
    if let Err(e) = std::fs::rename(&cur, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    if let Err(e) = std::fs::rename(&new, &cur) {
        let _ = std::fs::rename(&old, &cur); // put the working one back
        return Err(denied(e));
    }
    Ok(cur)
}

/// Quits the running overlay (if any), waits for it to go, and starts `exe`: the new version's overlay.
pub fn restart_overlay(exe: &Path) {
    unsafe {
        let h = FindWindowW(wide(crate::overlay::CLASS).as_ptr(), null());
        if !h.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            let p = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            PostMessageW(h, WM_CLOSE, 0, 0);
            if !p.is_null() {
                WaitForSingleObject(p, 5000); // (its single-instance lock goes with it, so the new one can start)
                CloseHandle(p);
            }
        }
    }
    let _ = std::process::Command::new(exe).spawn();
}

/// Deletes the exe an update left behind (it couldn't be deleted while it was running).
pub fn clean_up() {
    if let Ok(cur) = std::env::current_exe() {
        let _ = std::fs::remove_file(cur.with_extension("old"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real network path against GitHub: TLS, the API, and an asset download through GitHub's redirect to its
    /// file host. Needs the internet, so it runs only when asked: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_from_github() {
        let url = format!("https://api.github.com/repos/{}/releases/latest", update::REPO);
        let json = http_get(&url, "application/vnd.github+json").expect("the API answers");
        let v: serde_json::Value = serde_json::from_slice(&json).expect("JSON");
        assert!(v["tag_name"].as_str().is_some());
        let asset = v["assets"].as_array().and_then(|a| a.first()).expect("the release has an asset");
        let body = http_get(asset["browser_download_url"].as_str().unwrap(), "application/octet-stream").expect("the download works");
        assert_eq!(body.len() as u64, asset["size"].as_u64().unwrap(), "the whole file arrived");
        assert!(http_get("http://example.com/", "*/*").is_err(), "plain http is refused");
    }

    /// The latest published release would install: its exe downloads and carries a valid signature from our key for
    /// its own version. Run after each release: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn latest_release_is_signed() {
        let url = format!("https://api.github.com/repos/{}/releases/latest", update::REPO);
        let rel = update::parse_latest(&http_get(&url, "application/vnd.github+json").unwrap()).expect("a signed release");
        let exe = http_get(&rel.exe_url, "application/octet-stream").unwrap();
        let sig = http_get(&rel.sig_url, "application/octet-stream").unwrap();
        assert_eq!(update::verify(&exe, &String::from_utf8_lossy(&sig), update::PUBLIC_KEY, &rel.tag), Ok(()), "release {}", rel.tag);
    }
}
