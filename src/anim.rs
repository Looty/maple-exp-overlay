//! The small effects the overlay can show when EXP comes in: numbers that roll to their new value, a "+EXP" pop-up that
//! floats up and fades, and a snail that crawls along the top for each gain. Only the timing, easing and the sprite live
//! here (pure, so unit tested); the drawing is in the overlay.

/// How long a number takes to roll to its new value.
pub const COUNT_SECS: f64 = 0.35;
/// How long a "+EXP" pop-up lives.
pub const POP_SECS: f64 = 1.2;

/// A rainbow colour (0x00BBGGRR): `t` is the position on the colour wheel, 1.0 is a full turn.
pub fn rainbow(t: f64) -> u32 {
    let h = t.rem_euclid(1.0) * 6.0;
    let (i, f) = (h.floor() as u32, h.fract());
    let (s, v) = (0.9, 1.0);
    let (p, q, u) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i {
        0 => (v, u, p),
        1 => (q, v, p),
        2 => (p, v, u),
        3 => (p, q, v),
        4 => (u, p, v),
        _ => (v, p, q),
    };
    let c = |x: f64| (x * 255.0).round() as u32;
    c(r) | c(g) << 8 | c(b) << 16
}

/// Fast at first, settling at the end; 0..1 in, 0..1 out.
pub fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// A number that rolls up to a new value instead of jumping. A value that goes down (a new level, a reset) shows at once.
#[derive(Clone, Copy, Default, Debug)]
pub struct Eased {
    from: f64,
    to: f64,
    start: f64,
    set: bool,
}

impl Eased {
    /// The value to show at `now`, when the real value is `target`.
    pub fn at(&mut self, target: f64, now: f64) -> f64 {
        if !self.set {
            *self = Eased { from: target, to: target, start: now - COUNT_SECS, set: true };
        // (nothing to roll yet)
        } else if target != self.to {
            self.from = if target > self.to { self.value(now) } else { target };
            self.to = target;
            self.start = now;
        }
        self.value(now)
    }

    fn value(&self, now: f64) -> f64 {
        self.from + (self.to - self.from) * ease_out((now - self.start) / COUNT_SECS)
    }

    /// Still on its way to the target.
    pub fn busy(&self, now: f64) -> bool {
        self.set && now - self.start < COUNT_SECS
    }
}

/// The "+EXP" pop-up. Gains that come in while one is still showing add to it, so a fast grind makes one growing number
/// instead of a pile of them.
#[derive(Clone, Copy, Default, Debug)]
pub struct Pop {
    pub sum: f64,
    born: Option<f64>,
}

impl Pop {
    pub fn add(&mut self, amount: f64, now: f64) {
        if !self.alive(now) {
            self.sum = 0.0;
        }
        self.sum += amount;
        self.born = Some(now);
    }

    pub fn alive(&self, now: f64) -> bool {
        self.born.is_some_and(|b| now - b < POP_SECS)
    }

    /// (opacity 0..=255, how far up it has floated 0..1); None once it is gone.
    pub fn look(&self, now: f64) -> Option<(u8, f64)> {
        let t = (now - self.born?) / POP_SECS;
        (0.0..1.0).contains(&t).then(|| Pop::curve(t))
    }

    /// The pop-up's opacity and height at `t` (0 at the start, 1 at the end of its life): it floats up, and fades out over
    /// the last 60%.
    pub fn curve(t: f64) -> (u8, f64) {
        let fade = if t < 0.4 { 1.0 } else { 1.0 - (t - 0.4) / 0.6 };
        ((255.0 * fade.clamp(0.0, 1.0)) as u8, ease_out(t))
    }
}

/// How long a snail takes to crawl from the right end of the overlay to the left.
pub const WALK_SECS: f64 = 4.0;
/// Most snails crawling at once, and the shortest time between two starting.
const MAX_WALKERS: usize = 4;
const MIN_GAP_SECS: f64 = 0.8;

/// Screen pixels per sprite pixel at 96 dpi, in halves (3 is 1.5).
pub const SNAIL_PX: i32 = 3;

/// Where sprite pixel `i` starts, with `half` half-screen-pixels per sprite pixel: the next one starts where this one
/// ends, so odd sizes alternate between two widths and nothing is left between them.
pub fn cell_start(i: usize, half: i32) -> i32 {
    i as i32 * half / 2
}

/// The snails that crawl along the top of the overlay: one starts for each EXP gain, so nothing runs while nothing is
/// being gained.
#[derive(Clone, Default, Debug)]
pub struct Walkers {
    born: Vec<f64>,
}

impl Walkers {
    /// Starts a snail, unless there are enough already or one just started.
    pub fn spawn(&mut self, now: f64) -> bool {
        self.born.retain(|b| now - b < WALK_SECS);
        if self.born.len() >= MAX_WALKERS || self.born.last().is_some_and(|b| now - b < MIN_GAP_SECS) {
            return false;
        }
        self.born.push(now);
        true
    }

    pub fn alive(&self, now: f64) -> bool {
        self.born.iter().any(|b| now - b < WALK_SECS)
    }

    /// How far along each crawling snail is: 0 at the right end, 1 past the left end.
    pub fn progress(&self, now: f64) -> Vec<f64> {
        self.born.iter().map(|b| (now - b) / WALK_SECS).filter(|t| (0.0..1.0).contains(t)).collect()
    }
}

/// The snail's pixel art, 20x13, facing left, two frames (the eye stalks lean, the foot ripples). `.` is empty, `k`
/// outline, `c` shell, `d` shell spiral and shade, `h` shell highlight, `b` body, `e` eyes. Drawn from scratch for this
/// project.
pub const SPRITE_W: usize = 20;
pub const SPRITE_H: usize = 13;
pub const SPRITE: [[&str; SPRITE_H]; 2] = [
    [
        ".........kkkkkkkk...",
        ".......kkhhcccccckk.",
        "......khhccccddcccck",
        "kk.kk.kccdddccdcccck",
        "kek.kek.kcdcccdddcck",
        "kbk.kbk.kcdccddcdcck",
        ".kbkkbk.kccdddddccck",
        ".kbbbbbkkcccccccccck",
        ".kbbbbbbkddcccccccck",
        ".kbbbbbbbkkddddddkk.",
        ".kbbbbbbbbbbbbbbbbk.",
        ".kkbbbbbbbbbbbbbbkk.",
        "..kkkkkkkkkkkkkkkk..",
    ],
    [
        ".........kkkkkkkk...",
        ".......kkhhcccccckk.",
        "......khhccccddcccck",
        ".kk.kkkccdddccdcccck",
        "kek.kek.kcdcccdddcck",
        "kbk.kbk.kcdccddcdcck",
        ".kbkkbk.kccdddddccck",
        ".kbbbbbkkcccccccccck",
        ".kbbbbbbkddcccccccck",
        ".kbbbbbbbkkddddddkk.",
        ".kbbbbbbbbbbbbbbbbk.",
        ".kbbbbbbbbbbbbbbbbk.",
        "..kkkkkkkkkkkkkkkk..",
    ],
];

/// A sprite pixel's colour (0x00BBGGRR); None for empty.
pub fn sprite_color(c: u8) -> Option<u32> {
    let rgb = |r: u32, g: u32, b: u32| r | g << 8 | b << 16;
    match c {
        b'k' => Some(rgb(60, 35, 20)),
        b'c' => Some(rgb(235, 140, 50)),
        b'd' => Some(rgb(170, 90, 30)),
        b'h' => Some(rgb(255, 215, 150)),
        b'b' => Some(rgb(175, 220, 175)),
        b'e' => Some(rgb(30, 20, 15)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_roll_up_and_jump_down() {
        let mut e = Eased::default();
        assert_eq!(e.at(100.0, 0.0), 100.0); // the first value just shows
        let mid = e.at(200.0, 1.0);
        assert_eq!(mid, 100.0);
        assert!(e.at(200.0, 1.0 + COUNT_SECS / 2.0) > 150.0); // fast start
        assert!(e.busy(1.1));
        assert_eq!(e.at(200.0, 1.0 + COUNT_SECS), 200.0);
        assert!(!e.busy(2.0));
        assert_eq!(e.at(5.0, 3.0), 5.0); // a drop (new level) shows at once
    }

    #[test]
    fn pops_merge_then_fade() {
        let mut p = Pop::default();
        assert!(p.look(0.0).is_none());
        p.add(100.0, 1.0);
        p.add(50.0, 1.5);
        assert_eq!(p.sum, 150.0);
        assert_eq!(p.look(1.5).unwrap().0, 255);
        let (late, rise) = p.look(1.5 + POP_SECS * 0.9).unwrap();
        assert!(late < 120 && rise > 0.9);
        assert!(p.look(1.5 + POP_SECS).is_none());
        p.add(7.0, 10.0); // the old one was gone: starts over
        assert_eq!(p.sum, 7.0);
    }

    #[test]
    fn rainbow_walks_the_wheel() {
        assert_eq!(rainbow(0.0), 0x1919FF); // red (BGR order)
        assert_eq!(rainbow(1.0), rainbow(0.0)); // a full turn comes back
        assert_ne!(rainbow(0.0), rainbow(0.33));
        assert_ne!(rainbow(0.33), rainbow(0.66));
    }

    #[test]
    fn sprite_is_well_formed() {
        for frame in SPRITE {
            assert!(frame.iter().all(|row| row.len() == SPRITE_W));
            for row in frame {
                assert!(row.bytes().all(|c| c == b'.' || sprite_color(c).is_some()), "{row}");
            }
        }
        // the two frames differ only in the eye stalks (row 3) and the foot (row 11)
        for row in 0..SPRITE_H {
            assert_eq!(SPRITE[0][row] == SPRITE[1][row], row != 3 && row != 11, "row {row}");
        }
    }

    #[test]
    fn snails_crawl_across_and_are_limited() {
        let mut w = Walkers::default();
        assert!(!w.alive(0.0));
        assert!(w.spawn(1.0));
        assert!(!w.spawn(1.2)); // too soon after the last one
        assert!(w.spawn(2.0));
        assert_eq!(w.progress(1.0 + WALK_SECS / 2.0)[0], 0.5);
        assert!(w.alive(1.0 + WALK_SECS - 0.1));
        assert!(w.progress(2.0 + WALK_SECS + 0.1).is_empty());
        let mut w = Walkers::default();
        let started = (0..10).filter(|i| w.spawn(*i as f64 * 0.9)).count();
        assert!(started >= 4);
        assert!(w.progress(6.0).len() <= 4);
    }

    #[test]
    fn sprite_cells_tile_without_gaps() {
        for i in 0..SPRITE_W {
            assert!(cell_start(i + 1, SNAIL_PX) > cell_start(i, SNAIL_PX));
        }
        assert_eq!(cell_start(SPRITE_W, SNAIL_PX), SPRITE_W as i32 * SNAIL_PX / 2);
    }
}
