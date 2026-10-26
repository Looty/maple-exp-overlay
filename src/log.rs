//! Debug-build-only rolling log: the last `KEEP` lines, rewritten to `debug.log` in the settings folder.
//! In release builds `dlog!` expands to nothing, so none of this exists in the shipped exe.

#[cfg(debug_assertions)]
mod imp {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    const KEEP: usize = 100;
    static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
    static START: Mutex<Option<Instant>> = Mutex::new(None);

    pub fn push(msg: String) {
        let t = START.lock().unwrap().get_or_insert_with(Instant::now).elapsed().as_secs_f64();
        let mut lines = LINES.lock().unwrap();
        if lines.len() == KEEP {
            lines.pop_front();
        }
        lines.push_back(format!("[{t:9.3}] {msg}"));
        if let Some(dir) = maple_exp::config::dir() {
            let _ = std::fs::create_dir_all(&dir);
            let text = lines.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
            let _ = std::fs::write(dir.join("debug.log"), text);
        }
    }
}

#[cfg(debug_assertions)]
pub fn push(msg: String) {
    imp::push(msg);
}

#[cfg(debug_assertions)]
macro_rules! dlog {
    ($($arg:tt)*) => { $crate::log::push(format!($($arg)*)) };
}

#[cfg(not(debug_assertions))]
macro_rules! dlog {
    // (never runs, but keeps the values it mentions "used" so release builds don't warn)
    ($($arg:tt)*) => {
        if false {
            let _ = format!($($arg)*);
        }
    };
}

pub(crate) use dlog;
