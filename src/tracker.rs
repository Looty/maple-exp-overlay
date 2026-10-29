//! Turns a stream of bar readings into EXP/hr. Works from the percentage alone (so it also works when
//! the game hides the EXP number); uses the number too when it's there.

use crate::exp_table::{exp_to_next, level_for};
use crate::reader::Reading;
use std::collections::VecDeque;

const MAX_STEP: f64 = 5.0; // longest stretch of time one look can add (a stalled capture adds nothing)
const MAX_JUMP: f64 = 25.0; // a bigger percent jump in one look isn't farming: start over from it
/// Bars in the EXP/h graph: one per look at the bar, the newest last.
pub const SPARK_BARS: usize = 30;

pub struct Tracker {
    /// No gain for this many seconds: paused, and the idle time isn't counted.
    pub idle: f64,
    /// Length, in seconds, of the "last N minutes" rate.
    pub window: f64,
    /// A gain after this many seconds without EXP starts a fresh session (the gained totals and rates start over from
    /// that point, not counting that first gain). None = the session only ends with a reset.
    pub new_session_after: Option<f64>,
    last: Option<Reading>,
    pending: Option<Reading>,
    req: Option<f64>, // EXP needed for this level: from the table once the level is known, else estimated from number and percent
    /// The character's level, worked out from the EXP number, the percentage and the levelling table.
    level: Option<u32>,
    pub gained_pct: f64,
    pub gained_exp: f64,
    pub has_exp: bool,
    pub active: f64, // seconds spent farming
    last_tick: f64,
    last_gain: f64,
    gains: VecDeque<(f64, f64, f64)>, // (active time, exp, pct)
    /// The EXP/h (or %/h) after each of the last `SPARK_BARS` looks that read the bar, for the graph.
    rates: VecDeque<f32>,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Stats {
    pub pct: Option<f64>,
    pub exp_hr: Option<f64>,
    pub exp_5m: Option<f64>,
    pub pct_hr: Option<f64>,
    /// The character's level, once it is known (see `exp_table::level_for`).
    pub level: Option<u32>,
    /// Minutes to the next level at the whole session's pace.
    pub eta_min: Option<f64>,
    /// ...and at the pace of the last few minutes (see `Tracker::window`).
    pub eta_recent_min: Option<f64>,
    pub paused: bool,
    pub has_exp: bool,
    /// The EXP number on the bar now (None when the game shows only the percentage).
    pub exp_now: Option<u64>,
    pub gained_exp: Option<f64>,
    /// Percent of a level gained this session.
    pub gained_pct: f64,
    /// Seconds spent farming (idle stretches left out).
    pub active: f64,
    /// Seconds since the bar last changed (EXP came in, or the first reading); None before any reading.
    pub since_gain: Option<f64>,
    /// The rate after each of the last `SPARK_BARS` looks, oldest first: EXP/h when the bar shows the number, else
    /// %/h. None where there is no rate yet (the start of a session).
    pub spark: [Option<f32>; SPARK_BARS],
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            idle: 15.0,
            window: 300.0,
            new_session_after: None,
            last: None,
            pending: None,
            req: None,
            level: None,
            gained_pct: 0.0,
            gained_exp: 0.0,
            has_exp: false,
            active: 0.0,
            last_tick: 0.0,
            last_gain: 0.0,
            gains: VecDeque::new(),
            rates: VecDeque::new(),
        }
    }
}

impl Tracker {
    /// Starts a new session; the idle and window settings stay.
    pub fn reset(&mut self) {
        // (the level and the size of this level are still true in a new session)
        *self = Tracker {
            idle: self.idle,
            window: self.window,
            new_session_after: self.new_session_after,
            level: self.level,
            req: self.req,
            ..Tracker::default()
        };
    }

    /// One look at the bar at time `t` (seconds, monotonic); None if it couldn't be read. A look that read the bar
    /// adds a bar to the graph.
    pub fn observe(&mut self, t: f64, r: Option<Reading>) {
        let read = r.is_some();
        self.take(t, r);
        if let (true, Some(rate)) = (read, self.rate()) {
            self.rates.push_back(rate as f32);
            if self.rates.len() > SPARK_BARS {
                self.rates.pop_front();
            }
        }
    }

    fn take(&mut self, t: f64, r: Option<Reading>) {
        if self.last.is_some() && t - self.last_gain <= self.idle {
            self.active += (t - self.last_tick).clamp(0.0, MAX_STEP);
        }
        self.last_tick = t;
        let Some(r) = r else { return };
        let Some(last) = self.last else {
            if self.pending == Some(r) {
                self.accept(t, r);
            } else {
                self.pending = Some(r);
            }
            return;
        };
        if r == last {
            self.pending = None;
            return;
        }
        // a changed reading counts only when two looks in a row agree (one misread digit isn't EXP)
        if self.pending != Some(r) {
            self.pending = Some(r);
            return;
        }
        self.pending = None;
        if self.new_session_after.is_some_and(|after| t - self.last_gain > after) {
            // back after a long break: a fresh session, counting from this reading on
            self.gained_pct = 0.0;
            self.gained_exp = 0.0;
            self.has_exp = false;
            self.active = 0.0;
            self.gains.clear();
            self.rates.clear();
            self.last_tick = t;
            self.accept(t, r);
            return;
        }
        let was_idle = t - self.last_gain > self.idle;
        let (gain_pct, gain_exp) = self.delta(last, r);
        self.accept(t, r);
        if gain_pct > 0.0 {
            if was_idle {
                self.last_tick = t; // the gap wasn't farming
            }
            self.gained_pct += gain_pct;
            if let Some(e) = gain_exp {
                self.gained_exp += e;
                self.has_exp = true;
            }
            self.gains.push_back((self.active, gain_exp.unwrap_or(0.0), gain_pct));
            while self.gains.front().is_some_and(|g| g.0 < self.active - self.window) {
                self.gains.pop_front();
            }
        }
    }

    /// The session's rate now: EXP/h when the bar shows the number, else %/h; None for the first seconds.
    fn rate(&self) -> Option<f64> {
        (self.active >= 10.0).then(|| if self.has_exp { self.gained_exp } else { self.gained_pct } * 3600.0 / self.active)
    }

    fn accept(&mut self, t: f64, r: Reading) {
        self.last = Some(r);
        self.last_gain = t;
        // the table says which level this is when the EXP number and percentage fit exactly one of them
        if let Some(l) = r.exp.and_then(|e| level_for(e, r.pct)) {
            self.level = Some(l);
        }
        match self.level.and_then(exp_to_next) {
            Some(need) => self.req = Some(need as f64),
            None => {
                if let (Some(e), true) = (r.exp, r.pct >= 1.0) {
                    self.req = Some(e as f64 / (r.pct / 100.0));
                }
            }
        }
    }

    /// (percent gained, EXP gained) between two accepted readings; (0, None) when it isn't a gain.
    fn delta(&mut self, a: Reading, b: Reading) -> (f64, Option<f64>) {
        let d = b.pct - a.pct;
        if (0.0..=MAX_JUMP).contains(&d) {
            let exp = match (a.exp, b.exp) {
                (Some(x), Some(y)) if y >= x => Some((y - x) as f64),
                _ => self.req.map(|r| d / 100.0 * r),
            };
            (d, exp)
        } else if d < 0.0 && a.pct >= 70.0 && b.pct <= 30.0 {
            // level up: the rest of the old bar plus what's in the new one
            let pct = 100.0 - a.pct + b.pct;
            let exp = match (self.req, a.exp, b.exp) {
                (Some(req), Some(x), Some(y)) => Some(req - x as f64 + y as f64),
                (Some(req), _, _) => Some(pct / 100.0 * req),
                _ => None,
            };
            self.level = self.level.map(|l| (l + 1).min(99));
            self.req = self.level.and_then(exp_to_next).map(|n| n as f64);
            (pct, exp)
        } else {
            (0.0, None) // death, a big jump: just continue from the new value
        }
    }

    pub fn stats(&self, t: f64) -> Stats {
        let mut s = Stats {
            pct: self.last.map(|r| r.pct),
            has_exp: self.has_exp,
            exp_now: self.last.and_then(|r| r.exp),
            gained_exp: self.has_exp.then_some(self.gained_exp),
            gained_pct: self.gained_pct,
            active: self.active,
            since_gain: self.last.map(|_| (t - self.last_gain).max(0.0)),
            ..Default::default()
        };
        s.paused = self.last.is_some() && t - self.last_gain > self.idle;
        if self.active >= 10.0 {
            let per_hr = 3600.0 / self.active;
            let pct_hr = self.gained_pct * per_hr;
            s.pct_hr = Some(pct_hr);
            if self.has_exp {
                s.exp_hr = Some(self.gained_exp * per_hr);
                let span = self.active.min(self.window);
                let recent: f64 = self.gains.iter().filter(|g| g.0 >= self.active - self.window).map(|g| g.1).sum();
                s.exp_5m = Some(recent / span * 3600.0);
            }
            let recent_pct: f64 = self.gains.iter().filter(|g| g.0 >= self.active - self.window).map(|g| g.2).sum();
            let recent_hr = recent_pct / self.active.min(self.window) * 3600.0;
            // EXP left in the level, exactly, when the level is known and the bar shows the number; else the percent left
            let left_exp = match (self.level.and_then(exp_to_next), s.exp_now) {
                (Some(need), Some(now)) => Some(need.saturating_sub(now) as f64),
                _ => None,
            };
            let eta = |exp_rate: Option<f64>, pct_rate: f64| -> Option<f64> {
                match (left_exp, exp_rate) {
                    (Some(left), Some(rate)) if rate > 0.0 => Some(left / rate * 60.0),
                    _ if pct_rate > 0.0 => s.pct.map(|p| (100.0 - p) / pct_rate * 60.0),
                    _ => None,
                }
            };
            s.eta_min = eta(s.exp_hr, pct_hr);
            s.eta_recent_min = eta(s.exp_5m, recent_hr);
        }
        s.level = self.level;
        // (right-aligned: the newest look is the last bar)
        let skip = SPARK_BARS - self.rates.len();
        for (slot, &v) in s.spark[skip..].iter_mut().zip(&self.rates) {
            *slot = Some(v);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(exp: u64, pct: f64) -> Option<Reading> {
        Some(Reading { exp: Some(exp), pct })
    }

    #[test]
    fn steady_gain_and_level_up() {
        let mut t = Tracker::default();
        // 1000 EXP per 5 s at 1% per 1000 (req = 100_000), seen twice each so it is accepted
        let mut e = 50_000u64;
        let mut now = 0.0;
        for step in 0..30 {
            for _ in 0..2 {
                t.observe(now, rd(e, e as f64 / 1000.0));
                now += 1.0;
            }
            now += 3.0;
            if step < 29 {
                e += 1000;
            }
        }
        let s = t.stats(now);
        let hr = s.exp_hr.unwrap();
        assert!((hr - 720_000.0).abs() / 720_000.0 < 0.15, "{hr}");
        assert!(s.eta_min.unwrap() > 0.0);
        // level up: 99.5% -> 0.5%
        t.observe(now, rd(99_500, 99.5));
        t.observe(now + 1.0, rd(99_500, 99.5));
        t.observe(now + 2.0, rd(600, 0.5));
        let before = t.gained_exp;
        t.observe(now + 3.0, rd(600, 0.5));
        assert!(t.gained_exp > before + 900.0 && t.gained_exp < before + 1200.0);
    }

    #[test]
    fn single_misread_is_ignored_and_idle_pauses() {
        let mut t = Tracker::default();
        t.observe(0.0, rd(1000, 1.0));
        t.observe(1.0, rd(1000, 1.0));
        t.observe(2.0, rd(9000, 9.0)); // one odd look
        t.observe(3.0, rd(1000, 1.0));
        assert_eq!(t.gained_pct, 0.0);
        for i in 4..40 {
            t.observe(i as f64, rd(1000, 1.0));
        }
        assert!(t.stats(40.0).paused);
        assert!(t.active < 20.0, "idle time must not count: {}", t.active);
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn a_long_break_starts_a_fresh_session() {
        let mut t = Tracker { new_session_after: Some(300.0), ..Tracker::default() };
        let mut exp = 20_000u64;
        let rd = |e: u64| Some(Reading { exp: Some(e), pct: (e as f64 / 122_760.0 * 10_000.0).round() / 100.0 });
        for s in 0..120 {
            if s % 3 == 0 {
                exp += 165;
            }
            t.observe(s as f64, rd(exp));
        }
        assert!(t.stats(120.0).gained_exp.unwrap() > 4_000.0);
        // ten minutes of nothing (the same reading), then EXP again
        for s in 120..720 {
            t.observe(s as f64, rd(exp));
        }
        exp += 165;
        for s in 720..723 {
            t.observe(s as f64, rd(exp));
        }
        // the session started over at that reading: nothing gained yet, no farmed time, and the level is still known
        let st = t.stats(723.0);
        assert!(st.gained_exp.is_none() && st.gained_pct == 0.0 && st.active < 5.0, "{st:?}");
        assert_eq!(st.level, Some(32));
        // ...and it counts from there
        for s in 723..783 {
            if s % 3 == 0 {
                exp += 165;
            }
            t.observe(s as f64, rd(exp));
        }
        let g = t.stats(783.0).gained_exp.unwrap();
        assert!((2_800.0..4_000.0).contains(&g), "{g}");
    }

    #[test]
    fn graph_has_a_bar_per_look_with_the_rate_then() {
        let mut t = Tracker::default();
        let mut exp = 20_000u64;
        let rd = |e: u64| Some(Reading { exp: Some(e), pct: (e as f64 / 122_760.0 * 10_000.0).round() / 100.0 });
        // 100 EXP every 2 s (180k/h), a look every second; no rate (so no bar) for the first 10 s
        for s in 0..15 {
            if s % 2 == 0 && s > 0 {
                exp += 100;
            }
            t.observe(s as f64, rd(exp));
        }
        let g = t.stats(15.0).spark;
        let shown = g.iter().flatten().count();
        assert!((3..=6).contains(&shown) && g[SPARK_BARS - 1].is_some() && g[0].is_none(), "{g:?}");
        // a look that couldn't read the bar adds nothing; later ones fill the graph, the newest last
        t.observe(15.0, None);
        assert_eq!(t.stats(15.0).spark.iter().flatten().count(), shown);
        for s in 16..60 {
            if s % 2 == 0 {
                exp += 100;
            }
            t.observe(s as f64, rd(exp));
        }
        let g = t.stats(60.0).spark;
        assert!(g.iter().all(|v| v.is_some_and(|r| (120_000.0..240_000.0).contains(&r))), "{g:?}");
        // a reset forgets the bars
        t.reset();
        assert!(t.stats(61.0).spark.iter().all(|v| v.is_none()));
    }

    /// A ten-minute grind on a level of 122,760 EXP: a kill every 3 s worth 165 EXP (about 200k/h), a look every second.
    #[test]
    fn time_to_level_counts_down_during_a_session() {
        let req = 122_760u64;
        let mut t = Tracker::default();
        let mut exp = 20_000u64;
        let mut etas = Vec::new();
        for sec in 0..600 {
            if sec % 3 == 0 && sec > 0 {
                exp += 165;
            }
            let pct = (exp as f64 / req as f64 * 10_000.0).round() / 100.0;
            t.observe(sec as f64, Some(Reading { exp: Some(exp), pct }));
            if sec % 120 == 119 {
                let s = t.stats(sec as f64);
                etas.push((sec, s.eta_min.unwrap(), s.eta_recent_min.unwrap(), s.exp_hr.unwrap()));
            }
        }
        println!("{etas:#?}");
        let first = etas.first().unwrap();
        let last = etas.last().unwrap();
        assert!(last.1 < first.1 - 5.0, "session-average ETA should fall: {first:?} -> {last:?}");
        assert!(last.2 < first.2 - 5.0, "recent-rate ETA should fall: {first:?} -> {last:?}");
        assert!((last.3 - 198_000.0).abs() < 20_000.0, "EXP/h should be near 198k: {}", last.3);
    }
}
