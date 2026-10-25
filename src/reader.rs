//! Finds the EXP text in a screenshot and reads it.
//!
//! The bar shows "EXP. 38,849 [61.02%]": white digits, green brackets. The game draws a small pixel
//! font scaled to the window size, so each digit is rescaled to a standard height and compared with
//! saved examples (digits.bin, 300 patches taken from the original project's digits.npz). No OCR engine.

/// A screenshot: BGRA, 4 bytes per pixel (what GDI hands back).
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct Blob {
    i: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub cx: f32,
    pub wide: bool,
}

#[derive(Debug)]
pub struct Layout {
    pub gh: f32,
    pub cy: f32,
    pub exp: Vec<Cell>,
    pub pct: Vec<Cell>,
    /// (x0, y0, x1, y1) of the number and brackets (for a percent-only bar: from where the number would start)
    pub bbox: (i32, i32, i32, i32),
}

/// What the bar says. `exp` is None when the game shows only the percentage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub exp: Option<u64>,
    pub pct: f64,
}

pub struct Found {
    pub layout: Layout,
    v: Vec<f32>,
    w: usize,
    h: usize,
}

// ---------------------------------------------------------------- colour masks and blobs

/// The digits are pure white; the status panel behind them can be a light grey-blue (up to ~240), which a lower
/// cutoff would merge with the digits into one big blob.
const WHITE_MIN: i32 = 245;
/// ...but a smaller or blurrier picture has dimmer digits, so lower cutoffs are tried when that finds nothing.
const WHITE_TRIES: [i32; 3] = [245, 200, 140];

fn hsv(b: u8, g: u8, r: u8) -> (i32, i32, i32) {
    let (bf, gf, rf) = (b as f32, g as f32, r as f32);
    let v = bf.max(gf).max(rf);
    let m = bf.min(gf).min(rf);
    let d = v - m;
    let s = if v == 0.0 { 0.0 } else { 255.0 * d / v };
    let mut h = if d == 0.0 {
        0.0
    } else if v == rf {
        60.0 * (gf - bf) / d
    } else if v == gf {
        120.0 + 60.0 * (bf - rf) / d
    } else {
        240.0 + 60.0 * (rf - gf) / d
    };
    if h < 0.0 {
        h += 360.0;
    }
    ((h / 2.0).round() as i32, s.round() as i32, v as i32)
}

/// 8-connected components of a mask: (labels, blobs). Labels are 1-based, 0 = background.
fn blobs(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<Blob>) {
    let mut labels = vec![0u32; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        let id = out.len() as u32 + 1;
        labels[start] = id;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        while let Some(p) = stack.pop() {
            let (x, y) = (p % w, p / w);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let q = ny * w + nx;
                    if mask[q] && labels[q] == 0 {
                        labels[q] = id;
                        stack.push(q);
                    }
                }
            }
        }
        out.push(Blob { i: id, x: x0 as i32, y: y0 as i32, w: (x1 - x0 + 1) as i32, h: (y1 - y0 + 1) as i32 });
    }
    (labels, out)
}

/// True for '[' (stroke on the left), false for ']'.
fn left_heavy(labels: &[u32], stride: usize, b: &Blob) -> bool {
    let half = (b.w / 2).max(1);
    let (mut l, mut r) = (0, 0);
    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            if labels[y as usize * stride + x as usize] == b.i {
                if x < b.x + half {
                    l += 1;
                }
                if x >= b.x + b.w - half {
                    r += 1;
                }
            }
        }
    }
    l > r
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Digit positions. A blob as wide as several digits (touching digits) is split evenly.
fn cells(bs: &[Blob], gh: f32) -> Vec<Cell> {
    let pitch = 0.84 * gh;
    let mut out = Vec::new();
    for b in bs {
        let (bw, bh) = (b.w as f32, b.h as f32);
        if 0.95 * bh < bw && bw < 1.25 * gh {
            out.push(Cell { cx: b.x as f32 + bw / 2.0, wide: true }); // the '%' sign
            continue;
        }
        let n = (((bw + 0.1 * gh) / pitch).round() as i32).max(1);
        for k in 0..n {
            out.push(Cell { cx: b.x as f32 + (k as f32 + 0.5) * bw / n as f32, wide: false });
        }
    }
    out
}

/// The '.' of the label "EXP." just left of a '[': tells the EXP bar from HP/MP when there's no number.
fn exp_label_dot(wblobs: &[Blob], bracket: &Blob) -> Option<Blob> {
    let bh = bracket.h as f32;
    let base = (bracket.y + bracket.h) as f32;
    let mut dots: Vec<&Blob> = wblobs
        .iter()
        .filter(|b| {
            b.x + b.w <= bracket.x
                && (bracket.x - b.x) as f32 <= 3.0 * bh
                && b.h as f32 <= 0.45 * bh
                && b.w as f32 <= 0.45 * bh
                && ((b.y + b.h) as f32 - base).abs() <= 0.35 * bh
        })
        .collect();
    dots.sort_by_key(|b| -b.x);
    let tall = |b: &Blob| (1.15 * bh..=1.9 * bh).contains(&(b.h as f32)) && ((b.y + b.h) as f32 - base).abs() <= 0.35 * bh;
    for dot in dots {
        let under = wblobs.iter().find(|p| {
            tall(p)
                && p.x as f32 + 0.4 * p.w as f32 <= dot.x as f32
                && dot.x + dot.w <= p.x + p.w + 1
                && dot.y as f32 >= p.y as f32 + 0.6 * p.h as f32
        });
        let Some(p) = under else { continue };
        // two or more letters come before the dot: separate blobs, or touching ones in one wide blob
        if p.w as f32 >= 1.8 * bh || wblobs.iter().any(|e| tall(e) && (p.x - (e.x + e.w)) as f32 <= 0.6 * bh && e.x + e.w <= p.x + 1) {
            return Some(*dot);
        }
    }
    None
}

/// Locates the EXP text in a screenshot.
pub fn find_exp(f: &Frame) -> Option<Found> {
    WHITE_TRIES.iter().find_map(|&m| find_exp_at(f, m))
}

/// Finds the bar and reads it, trying each white cutoff until one gives a clear reading.
pub fn find_and_read(t: &Templates, f: &Frame) -> Option<(Found, Reading)> {
    WHITE_TRIES.iter().find_map(|&m| {
        let found = find_exp_at(f, m)?;
        let r = read(t, &found)?;
        Some((found, r))
    })
}

fn find_exp_at(f: &Frame, white_min: i32) -> Option<Found> {
    let (w, h) = (f.w, f.h);
    let n = w * h;
    let mut green = vec![false; n];
    let mut white = vec![false; n];
    let mut v = vec![0f32; n];
    for i in 0..n {
        let (b, g, r) = (f.bgra[i * 4], f.bgra[i * 4 + 1], f.bgra[i * 4 + 2]);
        let (hue, sat, val) = hsv(b, g, r);
        v[i] = val as f32;
        green[i] = sat >= 90 && val >= 110 && (26..=50).contains(&hue);
        white[i] = sat < 70 && val >= white_min;
    }
    let (glabels, gblobs) = blobs(&green, w, h);
    let (_, wblobs) = blobs(&white, w, h);
    let mut brackets: Vec<Blob> = gblobs.into_iter().filter(|b| b.h >= 5 && b.w as f32 <= 0.6 * b.h as f32).collect();
    brackets.sort_by_key(|b| b.x);

    let mut found: Vec<Layout> = Vec::new();
    for o in &brackets {
        let bh = o.h as f32;
        let cy = o.y as f32 + bh / 2.0;
        let on_line = |b: &Blob| (b.y as f32 + b.h as f32 / 2.0 - cy).abs() <= 0.3 * bh;
        if o.w >= 6 && !left_heavy(&glabels, w, o) {
            continue; // clearly a ']'
        }
        let Some(c) = brackets
            .iter()
            .find(|c| c.x > o.x + o.w && (c.x - o.x) as f32 <= 9.0 * bh && on_line(c) && (c.h as f32 - bh).abs() <= 0.25 * bh)
        else {
            continue;
        };
        if c.w >= 6 && left_heavy(&glabels, w, c) {
            continue; // clearly another '['
        }
        let line: Vec<&Blob> = wblobs.iter().filter(|b| on_line(b) && b.h as f32 >= 0.6 * bh).collect();
        let mut inside: Vec<Blob> = line.iter().filter(|b| b.x >= o.x + o.w && b.x + b.w <= c.x).map(|b| **b).collect();
        inside.sort_by_key(|b| b.x);
        if inside.len() < 2 || inside.iter().any(|b| b.h as f32 > 1.25 * bh) {
            continue;
        }
        // the EXP number ends right before '['; walk left until a gap or the taller label
        let dh = median(inside.iter().map(|b| b.h as f32).collect());
        let mut left: Vec<Blob> = line.iter().filter(|b| b.x + b.w <= o.x).map(|b| **b).collect();
        left.sort_by_key(|b| -b.x);
        let mut run: Vec<Blob> = Vec::new();
        let mut edge = o.x;
        for b in left {
            if (edge - (b.x + b.w)) as f32 > 2.0 * bh || b.h as f32 > 1.25 * bh || b.h as f32 > 1.15 * dh {
                break;
            }
            edge = b.x;
            run.push(b);
        }
        let digit_h = |bs: &[&Blob]| -> f32 {
            let hs: Vec<f32> = bs.iter().filter(|b| b.w as f32 <= 0.85 * b.h as f32).map(|b| b.h as f32).collect();
            if hs.is_empty() {
                bh
            } else {
                median(hs)
            }
        };
        if run.is_empty() {
            // No number: only the label "EXP." tells this from HP/MP.
            let Some(dot) = exp_label_dot(&wblobs, o) else { continue };
            let gh = digit_h(&inside.iter().collect::<Vec<_>>());
            found.push(Layout {
                gh,
                cy,
                exp: vec![],
                pct: cells(&inside, gh),
                bbox: ((dot.x as f32 + 0.84 * gh) as i32, (cy - bh) as i32, c.x + c.w, (cy + bh) as i32),
            });
            continue;
        }
        run.reverse();
        let all: Vec<&Blob> = run.iter().chain(inside.iter()).collect();
        let gh = digit_h(&all);
        found.push(Layout {
            gh,
            cy,
            exp: cells(&run, gh),
            pct: cells(&inside, gh),
            bbox: (run[0].x, (cy - bh) as i32, c.x + c.w, (cy + bh) as i32),
        });
    }
    // The status bar is the lowest line found; EXP is the right-most value on it.
    let low = found.iter().map(|l| l.cy).fold(f32::MIN, f32::max);
    let best = found.into_iter().filter(|l| l.cy >= low - 0.5 * l.gh).max_by_key(|l| l.bbox.0)?;
    Some(Found { layout: best, v, w, h })
}

// ---------------------------------------------------------------- digit templates

const CANON: f32 = 24.0;
const PH: usize = 28;
const PW: usize = 19;
const SHIFT: usize = 3;
const MIN_SCORE: f32 = 0.75;
const MIN_LEAD: f32 = 0.03;
const WIN: usize = PH * PW;
const PATCH_H: usize = PH + 2 * SHIFT;
const PATCH_W: usize = PW + 2 * SHIFT;

fn unit_rows(a: &mut [f32]) {
    for row in a.chunks_mut(WIN) {
        let mean = row.iter().sum::<f32>() / WIN as f32;
        let mut norm = 0.0;
        for x in row.iter_mut() {
            *x -= mean;
            norm += *x * *x;
        }
        let norm = norm.sqrt().max(1e-6);
        for x in row.iter_mut() {
            *x /= norm;
        }
    }
}

pub struct Templates {
    labels: Vec<u8>,
    mat: Vec<f32>,
}

impl Templates {
    pub fn builtin() -> Self {
        let raw: &[u8] = include_bytes!("digits.bin");
        let n = raw.len() / (1 + WIN * 4);
        let labels = raw[..n].to_vec();
        let mut mat: Vec<f32> = raw[n..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        unit_rows(&mut mat);
        Templates { labels, mat }
    }

    /// Best correlation per digit, trying every small shift of the patch.
    fn scores(&self, patch: &[f32]) -> [f32; 10] {
        let mut wins = vec![0f32; (2 * SHIFT + 1) * (2 * SHIFT + 1) * WIN];
        let mut k = 0;
        for dy in 0..=2 * SHIFT {
            for dx in 0..=2 * SHIFT {
                for y in 0..PH {
                    let src = (dy + y) * PATCH_W + dx;
                    wins[k * WIN + y * PW..k * WIN + (y + 1) * PW].copy_from_slice(&patch[src..src + PW]);
                }
                k += 1;
            }
        }
        unit_rows(&mut wins);
        let mut best = [-1f32; 10];
        for (t, row) in self.mat.chunks_exact(WIN).enumerate() {
            let d = self.labels[t] as usize;
            for win in wins.chunks_exact(WIN) {
                let s: f32 = win.iter().zip(row).map(|(a, b)| a * b).sum();
                if s > best[d] {
                    best[d] = s;
                }
            }
        }
        best
    }

    /// (digit, score, lead over the runner-up) for one digit position.
    fn matches(&self, fd: &Found, cell: &Cell) -> (u8, f32, f32) {
        let mut best = [-1f32; 10];
        for f in [1.0f32, 0.95, 1.05] {
            // allow for small errors in the size estimate; the first scale usually settles it
            let s = self.scores(&patch(fd, cell.cx, fd.layout.cy, fd.layout.gh * f));
            for d in 0..10 {
                best[d] = best[d].max(s[d]);
            }
            let (top, lead) = top_two(&best);
            if f == 1.0 && best[top] >= 0.92 && lead >= 0.12 {
                break;
            }
        }
        let (top, lead) = top_two(&best);
        (top as u8, best[top], lead)
    }
}

fn top_two(best: &[f32; 10]) -> (usize, f32) {
    let mut idx: Vec<usize> = (0..10).filter(|&d| best[d] > -1.0).collect();
    idx.sort_by(|&a, &b| best[b].partial_cmp(&best[a]).unwrap());
    match idx.as_slice() {
        [a, b, ..] => (*a, best[*a] - best[*b]),
        [a] => (*a, 1.0),
        [] => (0, 0.0),
    }
}

/// Brightness patch around one digit, rescaled so the digit is CANON px tall (bilinear, edges repeated).
fn patch(fd: &Found, cx: f32, cy: f32, gh: f32) -> Vec<f32> {
    let k = gh / CANON;
    let x0 = cx - (PW as f32 / 2.0 + SHIFT as f32) * k;
    let y0 = cy - (PH as f32 / 2.0 + SHIFT as f32) * k;
    let at = |x: i32, y: i32| -> f32 { fd.v[y.clamp(0, fd.h as i32 - 1) as usize * fd.w + x.clamp(0, fd.w as i32 - 1) as usize] };
    let mut out = vec![0f32; PATCH_H * PATCH_W];
    for y in 0..PATCH_H {
        for x in 0..PATCH_W {
            let (sx, sy) = (k * x as f32 + x0, k * y as f32 + y0);
            let (ix, iy) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - ix, sy - iy);
            let (ix, iy) = (ix as i32, iy as i32);
            out[y * PATCH_W + x] = at(ix, iy) * (1.0 - fx) * (1.0 - fy)
                + at(ix + 1, iy) * fx * (1.0 - fy)
                + at(ix, iy + 1) * (1.0 - fx) * fy
                + at(ix + 1, iy + 1) * fx * fy;
        }
    }
    out
}

fn read_digits(t: &Templates, fd: &Found, cs: &[&Cell]) -> Option<String> {
    let mut s = String::new();
    for c in cs {
        if c.wide {
            return None;
        }
        let (d, score, lead) = t.matches(fd, c);
        if score < MIN_SCORE || lead < MIN_LEAD {
            return None;
        }
        s.push((b'0' + d) as char);
    }
    Some(s)
}

/// (exp, pct) read with the digit templates, or None if any digit is unclear.
pub fn read(t: &Templates, fd: &Found) -> Option<Reading> {
    let lay = &fd.layout;
    let right = lay.bbox.2 as f32;
    // the digits inside the brackets: drops the '%' sign right before the ']'
    let pct_cells: Vec<&Cell> = lay.pct.iter().filter(|c| right - c.cx > 1.5 * lay.gh).collect();
    let exp_cells: Vec<&Cell> = lay.exp.iter().collect();
    let exp_s = read_digits(t, fd, &exp_cells);
    let pct_s = read_digits(t, fd, &pct_cells)?;
    if (!exp_cells.is_empty() && exp_s.is_none()) || !(3..=5).contains(&pct_s.len()) {
        return None;
    }
    if exp_s.as_ref().is_some_and(|e| e.len() > 1 && e.starts_with('0')) || (pct_s.len() > 3 && pct_s.starts_with('0')) {
        return None; // a leading 0: some digits were cut off
    }
    Some(Reading { exp: exp_s.map(|e| e.parse().unwrap()), pct: pct_s.parse::<f64>().unwrap() / 100.0 })
}

/// A plain-words account of what's in a capture, for when the bar isn't found or read.
pub fn diagnose(t: &Templates, f: &Frame) -> String {
    let n = f.w * f.h;
    let (mut green, mut white, mut dark) = (vec![false; n], 0usize, 0usize);
    for i in 0..n {
        let (b, g, r) = (f.bgra[i * 4], f.bgra[i * 4 + 1], f.bgra[i * 4 + 2]);
        let (hue, sat, val) = hsv(b, g, r);
        green[i] = sat >= 90 && val >= 110 && (26..=50).contains(&hue);
        white += (sat < 70 && val >= 140) as usize;
        dark += (val < 12) as usize;
    }
    let size = format!("{}x{}", f.w, f.h);
    if dark == n {
        return format!(
            "capture is black|capture {size} is all black: the game is fullscreen-exclusive or protected; use borderless/windowed"
        );
    }
    if let Some((found, r)) = find_and_read(t, f) {
        return format!("area OK|OK {size}: {r:?} (digit height {})", found.layout.gh);
    }
    match find_exp(f) {
        Some(found) => {
            format!("digits unclear|bar found in {size} but the digits are unclear (digit height {}): box may cut a digit or something covers it", found.layout.gh)
        }
        None => {
            let (_, gb) = blobs(&green, f.w, f.h);
            let brackets = gb.iter().filter(|b| b.h >= 5 && b.w as f32 <= 0.6 * b.h as f32).count();
            if brackets == 0 {
                format!("no [ ] seen: box wrong/covered|capture {size}: no green [ ] brackets seen ({white} white px). The box must hold the whole 'EXP. 123 [45.67%]' unobscured (taskbar? another window?)")
            } else {
                format!("text cut off: widen the box|capture {size}: {brackets} green bracket(s) but no matching number/EXP. label: the box probably cuts the text")
            }
        }
    }
}

/// Every bracket-like green blob and every white blob in a capture, for working out why a bar isn't found.
pub fn debug_blobs(f: &Frame) -> String {
    let n = f.w * f.h;
    let (mut green, mut white) = (vec![false; n], vec![false; n]);
    for i in 0..n {
        let (hue, sat, val) = hsv(f.bgra[i * 4], f.bgra[i * 4 + 1], f.bgra[i * 4 + 2]);
        green[i] = sat >= 90 && val >= 110 && (26..=50).contains(&hue);
        white[i] = sat < 70 && val >= WHITE_MIN;
    }
    let (_, g) = blobs(&green, f.w, f.h);
    let (_, wb) = blobs(&white, f.w, f.h);
    let mut s = String::from("green blobs (x,y,w,h):");
    for b in &g {
        s += &format!(" ({},{},{},{})", b.x, b.y, b.w, b.h);
    }
    s += "\nwhite blobs (x,y,w,h):";
    for b in &wb {
        s += &format!(" ({},{},{},{})", b.x, b.y, b.w, b.h);
    }
    s
}
