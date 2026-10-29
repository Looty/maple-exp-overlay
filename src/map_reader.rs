//! `--read-map <fingerprint>`: reads the map name once, with Windows' OCR, and keeps it in `maps.json` under the
//! fingerprint the overlay saw. Its own short process: the overlay starts it only for a map it has no text for yet,
//! so the OCR engine and the 16-bit capture never live in the overlay, and a map is read once, not every look.

use crate::capture;
use crate::ocr;
use maple_exp::config::Settings;
use maple_exp::map::{self, Maps};
use maple_exp::reader::Frame;

/// What one reading found: the text at two sizes, and the picture they came from.
pub struct Reading {
    pub frame: Option<Frame>,
    pub small: Option<Vec<String>>,
    pub large: Option<Vec<String>>,
}

impl Reading {
    /// Both sizes read the same text: sure enough to keep for good.
    pub fn agreed(&self) -> Option<&[String]> {
        match (&self.small, &self.large) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    }
}

/// Spaces tidied ("West  Rocky" -> "West Rocky"); empty lines dropped.
fn tidy(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect()
}

/// Looks at the map area: the game window's picture in true colours (right with HDR on too), else the screen.
pub fn read(area: [i32; 4]) -> Reading {
    let [x, y, w, h] = area;
    let frame = unsafe { capture::capture_true_colour(x, y, w, h).or_else(|| capture::capture_screen(x, y, w, h)) };
    let Some(m) = frame.as_ref().and_then(map::mask) else { return Reading { frame, small: None, large: None } };
    let f = frame.as_ref().unwrap();
    let at = |scale: f32| ocr::lines(&map::ocr_picture(f, &m, scale)).map(tidy).filter(|l| !l.is_empty());
    let (small, large) = (at(1.0), at(1.5));
    Reading { frame, small, large }
}

pub fn run() {
    let Some(fp) = std::env::args().skip_while(|a| a != "--read-map").nth(1) else { return };
    let Some(area) = Settings::load().map_region else { return };
    let r = read(area);
    let mut maps = Maps::load();
    match (r.agreed(), r.small.as_ref().or(r.large.as_ref())) {
        (Some(text), _) => {
            maps.names.insert(fp.clone(), text.join("\n"));
            maps.guesses.remove(&fp);
        }
        // the two sizes disagree: show the reading, but try again next time rather than keep a wrong name for good
        (None, Some(guess)) => {
            maps.guesses.insert(fp, guess.join("\n"));
        }
        (None, None) => return,
    }
    maps.save(); // (the overlay sees the file change and shows it)
}
