//! The map you're on, from the minimap's header ("Victoria Road" / "West Rocky Mountain II").
//!
//! There is no OCR here and the digit shapes can't read letters, so a map is recognised by the shape of its name: the
//! game draws it in a fixed, unsmoothed light-grey font with a dark outline, so the same map always gives the same
//! pixels. The shape is taken from the outline: with HDR on, a screen capture lifts the game's colours so far that the
//! letters and the light panel behind them both come out pure white, and only the outline still shows where the
//! letters are. Only those pixels count (not the gradient), cropped to where they are, so a box selected a little
//! differently still gives the same fingerprint. The user names each new map once; the names live in `maps.json`.
//!
//! The fingerprint depends on the game's size and UI scale, like the EXP area: at another size, maps are new again.

use crate::config;
use crate::reader::Frame;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Fewer outline pixels than this: no name there (the minimap is closed, or something else is in the box).
const MIN_TEXT_PX: usize = 20;
/// How far around a pixel to look for the light letter (or panel) it is the outline of: the outline is one game pixel
/// wide, so two screen pixels at twice the size, and its outer side touches only the panel.
const REACH: usize = 2;
/// An outline pixel is darker than the lightest pixel within `REACH` by at least this much of that pixel's value
/// (and at least `MIN_DROP`). The panel's own gradient changes far less than that over a few pixels.
const DROP_RATIO: f32 = 0.1;
const MIN_DROP: i32 = 20;
/// Empty columns between two groups of marks (the icon and the name; letters and words sit closer than this).
const GROUP_GAP: usize = 12;
/// A group with more than VIVID_PERCENT % of strongly coloured pixels (channels further apart than VIVID) is the icon.
const VIVID: u8 = 140;
const VIVID_PERCENT: usize = 8;
/// Fewer marks than this in a group: not a word (the smallest, a short "I", has more).
const MIN_GROUP_PX: usize = 40;

/// The outline of the map name's letters in a picture, cropped to where it is.
pub struct Mask {
    /// where it sits in the picture it was found in
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
}

/// Finds the outline of the name's letters in `f`: pixels clearly darker than the lightest pixel near them. That
/// holds the same in a normal capture and in a washed-out one (HDR on), where the letters and the panel both turn
/// white but the outline only turns light blue. None when there is no name to see.
pub fn mask(f: &Frame) -> Option<Mask> {
    let (w, h) = (f.w, f.h);
    // each pixel's darkest channel (the outline is blue, so its red is where it is darkest)
    let lo: Vec<u8> = f.bgra.chunks_exact(4).map(|p| p[0].min(p[1]).min(p[2])).collect();
    // the lightest within REACH (a square), done as a row pass then a column pass
    let mut row_max = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            row_max[y * w + x] = (x.saturating_sub(REACH)..=(x + REACH).min(w - 1)).map(|i| lo[y * w + i]).max().unwrap_or(0);
        }
    }
    let mut near_max = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            near_max[y * w + x] = (y.saturating_sub(REACH)..=(y + REACH).min(h - 1)).map(|j| row_max[j * w + x]).max().unwrap_or(0);
        }
    }
    let raw = |x: usize, y: usize| {
        let (v, top) = (lo[y * w + x] as i32, near_max[y * w + x] as i32);
        top - v >= MIN_DROP.max((top as f32 * DROP_RATIO) as i32)
    };
    // a column or row that is marked nearly end to end is the edge of the window's frame caught in the box, not letters
    let col_line: Vec<bool> = (0..f.w).map(|x| (0..f.h).filter(|&y| raw(x, y)).count() * 10 >= f.h * 9).collect();
    // the panel is only as wide as the name, so a box made for a long name reaches past a short one's panel into the
    // game, which moves: stop at the panel's right edge (its frame, the first such line past the start)
    let edge = (w / 5..w).find(|&x| col_line[x]).unwrap_or(w);
    // (and its top and bottom bands run across the panel, not the whole box)
    let row_line: Vec<bool> = (0..f.h).map(|y| (0..edge).filter(|&x| raw(x, y)).count() * 10 >= edge * 9).collect();
    let marked = |x: usize, y: usize| x < edge && !col_line[x] && !row_line[y] && raw(x, y);
    // the region's icon (left of the name) has sharp edges too: split the marks into groups of columns with a clear
    // gap between them, and leave out the strongly coloured groups (the icon is bright yellow and blue; the letters
    // are grey with a blue outline, nowhere near as saturated)
    let used: Vec<bool> = (0..w).map(|x| (0..h).any(|y| marked(x, y))).collect();
    let mut keep = vec![false; w];
    let mut x = 0;
    while x < w {
        if !used[x] {
            x += 1;
            continue;
        }
        let (start, mut end, mut gap) = (x, x, 0);
        while x < w && gap < GROUP_GAP {
            if used[x] {
                (end, gap) = (x, 0);
            } else {
                gap += 1;
            }
            x += 1;
        }
        let area = (end - start + 1) * h;
        let vivid = (start..=end)
            .flat_map(|x| (0..h).map(move |y| (x, y)))
            .filter(|&(x, y)| {
                let p = &f.bgra[(y * w + x) * 4..][..3];
                p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) > VIVID
            })
            .count();
        // (a group of only a few marks is a speck of the panel's frame, such as its corner, not a word)
        let marks = (start..=end).flat_map(|x| (0..h).map(move |y| (x, y))).filter(|&(x, y)| marked(x, y)).count();
        if vivid * 100 < area * VIVID_PERCENT && marks >= MIN_GROUP_PX {
            keep[start..=end].iter_mut().for_each(|k| *k = true);
        }
    }
    let text = |x: usize, y: usize| keep[x] && marked(x, y);
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0, 0, 0);
    for y in 0..f.h {
        for x in 0..f.w {
            if text(x, y) {
                (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
            }
        }
    }
    if n < MIN_TEXT_PX {
        return None;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let bits = (0..w * h).map(|i| text(x0 + i % w, y0 + i / w)).collect();
    Some(Mask { x: x0, y: y0, w, h, bits })
}

impl Mask {
    /// FNV-1a over the size and the letters, eight pixels a byte.
    pub fn fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut put = |b: u8| h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        for b in (self.w as u32).to_le_bytes().into_iter().chain((self.h as u32).to_le_bytes()) {
            put(b);
        }
        for row in self.bits.chunks(self.w) {
            for byte in row.chunks(8) {
                put(byte.iter().enumerate().fold(0u8, |acc, (i, &b)| acc | (b as u8) << i));
            }
        }
        h
    }
}

/// The name's part of `f` (cropped to its letters, icon left out), grey, scaled by `scale` (smoothly) and padded:
/// what Windows' OCR reads best. Windows OCR misreads the game's pixel font at its own blocky size; a plain grey
/// picture of it with room around it, at 1 to 1.5 times its size, reads cleanly.
pub fn ocr_picture(f: &Frame, m: &Mask, scale: f32) -> Frame {
    const MARGIN: usize = 4; // (around the letters, from the picture)
    const PAD: usize = 24; // (white, added around that)
    let (x0, y0) = (m.x.saturating_sub(MARGIN), m.y.saturating_sub(MARGIN));
    let (x1, y1) = ((m.x + m.w + MARGIN).min(f.w), (m.y + m.h + MARGIN).min(f.h));
    let (cw, ch) = (x1 - x0, y1 - y0);
    let grey: Vec<f32> = (0..cw * ch)
        .map(|i| {
            let p = &f.bgra[((y0 + i / cw) * f.w + x0 + i % cw) * 4..][..3];
            (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0
        })
        .collect();
    let (sw, sh) = (((cw as f32 * scale) as usize).max(1), ((ch as f32 * scale) as usize).max(1));
    let (w, h) = (sw + 2 * PAD, sh + 2 * PAD);
    let mut bgra = vec![255u8; w * h * 4];
    for y in 0..sh {
        for x in 0..sw {
            // bilinear: smooth steps help the OCR more than sharp pixel edges
            let (fx, fy) = (((x as f32 + 0.5) / scale - 0.5).max(0.0), ((y as f32 + 0.5) / scale - 0.5).max(0.0));
            let (ax, ay) = ((fx as usize).min(cw - 1), (fy as usize).min(ch - 1));
            let (bx, by) = ((ax + 1).min(cw - 1), (ay + 1).min(ch - 1));
            let (tx, ty) = (fx - ax as f32, fy - ay as f32);
            let g = |x: usize, y: usize| grey[y * cw + x];
            let v = (g(ax, ay) * (1.0 - tx) + g(bx, ay) * tx) * (1.0 - ty) + (g(ax, by) * (1.0 - tx) + g(bx, by) * tx) * ty;
            let i = ((y + PAD) * w + x + PAD) * 4;
            let v = v.round().clamp(0.0, 255.0) as u8;
            bgra[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    Frame { w, h, bgra }
}

/// The fingerprint of the map name in `f`, or None when there is no name to see.
pub fn fingerprint(f: &Frame) -> Option<u64> {
    mask(f).map(|m| m.fingerprint())
}

/// The fingerprint as it is written in files.
pub fn hex(fp: u64) -> String {
    format!("{fp:016x}")
}

/// Looks in a row that must agree before the map shown changes (to a new one, or to none). The overlay makes them a
/// fifth of a second apart once something changed, so a new map shows within about half a second, while a cursor
/// or a name tag passing over the minimap, or the fade between maps, doesn't hold still long enough to count.
const LOOKS_TO_CHANGE: u32 = 3;

/// Which map is showing, steadied (see `LOOKS_TO_CHANGE`).
#[derive(Default, Debug)]
pub struct MapWatch {
    pub current: Option<u64>,
    candidate: Option<(Option<u64>, u32)>,
}

impl MapWatch {
    /// Starting from a map already known to be showing (the last one seen, at start-up).
    pub fn from(current: Option<u64>) -> MapWatch {
        MapWatch { current, candidate: None }
    }

    /// Something else than the current map was just seen and waits to be confirmed (look again soon).
    pub fn unsettled(&self) -> bool {
        self.candidate.is_some()
    }

    /// One look; true when `current` changed.
    pub fn see(&mut self, fp: Option<u64>) -> bool {
        if fp == self.current {
            self.candidate = None;
            return false;
        }
        let seen = match self.candidate {
            Some((c, k)) if c == fp => k + 1,
            _ => 1,
        };
        if seen >= LOOKS_TO_CHANGE {
            self.current = fp;
            self.candidate = None;
            return true;
        }
        self.candidate = Some((fp, seen));
        false
    }
}

/// The map names read so far (by `--read-map`, with Windows' OCR): fingerprint (hex) -> the header's text, one line
/// per line ("Victoria Road\nWest Rocky Mountain II"). In `maps.json` next to the settings.
#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Maps {
    /// Read the same at two sizes: kept for good.
    pub names: BTreeMap<String, String>,
    /// Read differently at the two sizes: shown, but read again the next time the map comes up.
    pub guesses: BTreeMap<String, String>,
}

pub fn path() -> Option<PathBuf> {
    config::dir().map(|d| d.join("maps.json"))
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// The map last seen, so a restarted overlay shows it at once (until a look says otherwise). A file of its own,
/// written only by the overlay, so it never races `--read-map` writing `maps.json`.
fn last_path() -> Option<PathBuf> {
    config::dir().map(|d| d.join("map-last.txt"))
}

pub fn load_last() -> Option<u64> {
    u64::from_str_radix(std::fs::read_to_string(last_path()?).ok()?.trim(), 16).ok()
}

pub fn save_last(fp: u64) {
    if let Some(p) = last_path() {
        let _ = std::fs::write(p, hex(fp));
    }
}

impl Maps {
    pub fn load() -> Maps {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        if let Ok(t) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    /// The header's text for a map, and whether it is for good (else a guess, to be read again).
    pub fn text(&self, fp: u64) -> Option<(&str, bool)> {
        let k = hex(fp);
        self.names.get(&k).map(|t| (t.as_str(), true)).or_else(|| self.guesses.get(&k).map(|t| (t.as_str(), false)))
    }

    /// What the Map item shows: the map's own line (the header's last: "West Rocky Mountain II", the region above it).
    pub fn name(&self, fp: u64) -> Option<&str> {
        self.text(fp).and_then(|(t, _)| t.lines().last())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: [u8; 3] = [231, 231, 231];

    /// A light-blue panel with a gradient that depends on `shade`, and a little "name" of text pixels drawn at (ox, oy)
    /// with a dark outline.
    fn panel(w: usize, h: usize, shade: u8, ox: usize, oy: usize, word: &[&str]) -> Frame {
        let mut bgra = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for _ in 0..w {
                let v = shade.saturating_add((y * 2) as u8);
                bgra.extend_from_slice(&[229, 210u8.min(v.max(150)), 163, 255]);
            }
        }
        let mut f = Frame { w, h, bgra };
        for (dy, line) in word.iter().enumerate() {
            for (dx, c) in line.chars().enumerate() {
                let i = ((oy + dy) * w + ox + dx) * 4;
                match c {
                    '#' => f.bgra[i..i + 3].copy_from_slice(&TEXT),
                    'o' => f.bgra[i..i + 3].copy_from_slice(&[179, 135, 93]),
                    _ => {}
                }
            }
        }
        f
    }

    const WORD: [&str; 6] = ["o###o###o", "o#o#o#o#o", "o###o###o", "o#ooo#o#o", "o#ooo###o", "#########"];

    #[test]
    fn the_same_name_gives_the_same_fingerprint_wherever_it_sits() {
        let a = fingerprint(&panel(60, 20, 160, 5, 4, &WORD)).expect("a name");
        let b = fingerprint(&panel(70, 24, 190, 30, 10, &WORD)).expect("a name");
        assert_eq!(a, b, "another box and another gradient behind it: still the same map");
        // one pixel of the letters different: another map
        let mut other = WORD;
        other[1] = "o#o#o#o##";
        assert_ne!(a, fingerprint(&panel(60, 20, 160, 5, 4, &other)).unwrap());
        // no letters (the minimap closed): nothing
        assert_eq!(fingerprint(&panel(60, 20, 160, 5, 4, &[])), None);
    }

    #[test]
    fn the_game_past_a_short_names_panel_is_left_out() {
        // a wide box: the panel (with its frame line) and, past it, the game, which moves between looks
        let with_game = |seed: usize| {
            let mut f = panel(90, 20, 160, 5, 4, &WORD);
            for y in 0..f.h {
                let i = (y * f.w + 40) * 4;
                f.bgra[i..i + 3].copy_from_slice(&[137, 106, 74]); // the panel's dark-blue right edge
                for x in 42..f.w {
                    let i = (y * f.w + x) * 4;
                    let v = ((x * 31 + y * 17 + seed * 7) % 256) as u8;
                    f.bgra[i..i + 3].copy_from_slice(&[v, v.wrapping_mul(3), v.wrapping_add(90)]);
                }
            }
            f
        };
        let alone = fingerprint(&panel(60, 20, 160, 5, 4, &WORD));
        assert_eq!(fingerprint(&with_game(1)), alone);
        assert_eq!(fingerprint(&with_game(2)), alone);
    }

    #[test]
    fn a_washed_out_capture_reads_the_same() {
        // HDR on: Windows lifts every colour channel the same way, so the letters stay grey but sit at another level
        let normal = panel(60, 20, 160, 5, 4, &WORD);
        let mut washed = panel(60, 20, 160, 5, 4, &WORD);
        for px in washed.bgra.chunks_exact_mut(4) {
            for c in &mut px[..3] {
                *c = (255.0 * (*c as f32 / 255.0).powf(0.45)) as u8;
            }
        }
        assert_eq!(fingerprint(&normal), fingerprint(&washed));
        // and the picture for the OCR is the name's part alone, grey, scaled and padded
        let m = mask(&normal).unwrap();
        let p = ocr_picture(&normal, &m, 1.5);
        assert!(p.w > (m.w as f32 * 1.5) as usize + 40 && p.h > (m.h as f32 * 1.5) as usize + 40);
        assert!(p.bgra.chunks(4).all(|px| px[0] == px[1] && px[1] == px[2]), "grey");
    }

    #[test]
    fn a_map_counts_after_three_agreeing_looks() {
        let mut w = MapWatch::default();
        assert!(!w.see(Some(1)) && w.unsettled());
        assert!(!w.see(Some(1)));
        assert!(w.see(Some(1)) && w.current == Some(1) && !w.unsettled());
        // a cursor over the name for a look or two changes nothing
        assert!(!w.see(Some(9)) && !w.see(Some(9)));
        assert!(!w.see(Some(1)) && !w.unsettled());
        assert!(!w.see(Some(9)) && w.current == Some(1));
        // the minimap closed: gone after three looks without a name
        assert!(!w.see(None) && !w.see(None));
        assert!(w.see(None) && w.current.is_none());
    }

    #[test]
    fn names_are_kept_by_fingerprint() {
        let mut m = Maps::default();
        m.names.insert(hex(42), "Victoria Road\nWest Rocky Mountain II".into());
        m.guesses.insert(hex(7), "Ant Tunnel I".into());
        // the map's own line, from a sure reading or a guess
        assert_eq!(m.name(42), Some("West Rocky Mountain II"));
        assert_eq!(m.text(42).map(|t| t.1), Some(true));
        assert_eq!((m.name(7), m.text(7).map(|t| t.1)), (Some("Ant Tunnel I"), Some(false)));
        assert_eq!(m.name(1), None);
        let back: Maps = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }
}
