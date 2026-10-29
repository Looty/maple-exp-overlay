//! The layout editor's geometry and moves, kept free of any UI code so they can be tested: where each item's chip
//! sits, which chip is under the pointer, where a dragged chip would land, and how the item list changes when it does.

use crate::config::Item;

/// Sizes in logical pixels, from the overlay's font size.
#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub font_px: f32,
    pub chip_h: f32,
    pub gap: f32,
    pub pad: f32,
}

impl Geo {
    pub fn new(font_pt: i32) -> Geo {
        let font_px = font_pt as f32 * 4.0 / 3.0;
        Geo { font_px, chip_h: (font_px * 1.9).max(26.0), gap: 8.0, pad: 12.0 }
    }

    fn row_y(&self, row: usize) -> f32 {
        self.pad + row as f32 * (self.chip_h + self.gap)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Index into the item list.
    pub idx: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What each item looks like on the overlay, as a sample for its chip.
pub fn sample(id: &str) -> &'static str {
    match id {
        "exp_hr" => "EXP/h  21.1K",
        "pct_hr" => "12.3 %/h",
        "exp_window" => "5 min  21.1K",
        "eta" => "Lv in 3h 01m",
        "level" => "Lv 32",
        "pct" => "20.25%",
        "exp_now" => "EXP 24,855",
        "gained" => "Gained 12.3K",
        "gained_pct" => "Gained 1.23%",
        "elapsed" => "Time 1:12:30",
        "spark" => "▂▃▅▃▆▇",
        "map" => "West Rocky Mountain II",
        _ => "",
    }
}

/// The items grouped into rows (indices), as the overlay lays them out: an item with `newline` starts a row.
pub fn rows(items: &[Item]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if it.newline || out.is_empty() {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(i);
    }
    out
}

/// The widths of each row's pieces, given as (own width, fills), once the `fill` ones take the room their row leaves
/// of the widest row (shared equally when a row has several).
pub fn stretch(rows: &[Vec<(f32, bool)>], gap: f32) -> Vec<Vec<f32>> {
    let row_w = |r: &Vec<(f32, bool)>| r.iter().map(|p| p.0).sum::<f32>() + gap * r.len().saturating_sub(1) as f32;
    let widest = rows.iter().map(row_w).fold(0.0, f32::max);
    rows.iter()
        .map(|r| {
            let n = r.iter().filter(|p| p.1).count();
            let extra = if n > 0 { (widest - row_w(r)) / n as f32 } else { 0.0 };
            r.iter().map(|&(w, fill)| if fill { w + extra } else { w }).collect()
        })
        .collect()
}

/// Every chip's rectangle, and the editor's height (with one empty row below, to drop a chip on for a new row).
pub fn place(items: &[Item], g: &Geo) -> (Vec<Placed>, f32) {
    let mut out = Vec::new();
    let r = rows(items);
    let own = |i: usize| {
        // (the graph's block glyphs are about as wide as the font is tall; letters about 0.6 of it)
        let em = if items[i].id == "spark" { 1.0 } else { 0.6 };
        22.0 + sample(&items[i].id).chars().count() as f32 * g.font_px * em
    };
    let widths = stretch(&r.iter().map(|row| row.iter().map(|&i| (own(i), items[i].fill)).collect()).collect::<Vec<_>>(), g.gap);
    for (ri, row) in r.iter().enumerate() {
        let mut x = g.pad;
        for (&i, &w) in row.iter().zip(&widths[ri]) {
            out.push(Placed { idx: i, x, y: g.row_y(ri), w, h: g.chip_h });
            x += w + g.gap;
        }
    }
    (out, g.row_y(r.len() + 1) - g.gap + g.pad - 4.0)
}

/// The item whose chip is under (x, y).
pub fn hit(placed: &[Placed], x: f32, y: f32) -> Option<usize> {
    placed.iter().find(|p| x >= p.x && x <= p.x + p.w && y >= p.y && y <= p.y + p.h).map(|p| p.idx)
}

/// Where dropping the chip of item `dragged` at (x, y) would put it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// The row it joins; with `new_row`, the row a new one is made in front of (the row count = after the last).
    pub row: usize,
    pub col: usize,
    /// The insertion marker: (x, y, width, height). Upright inside a row, flat between rows.
    pub caret: (f32, f32, f32, f32),
    pub new_row: bool,
}

pub fn target(items: &[Item], placed: &[Placed], g: &Geo, dragged: usize, x: f32, y: f32) -> Target {
    let r = rows(items);
    let step = g.chip_h + g.gap;
    // near the line between two rows (or above the first, or below the last): a row of its own there
    let line = |k: usize| g.pad - g.gap / 2.0 + k as f32 * step;
    let k = (((y - line(0)) / step).round().max(0.0) as usize).min(r.len());
    let edge = (g.chip_h * 0.3).max(6.0) + g.gap / 2.0;
    if (y - line(k)).abs() <= edge || y < line(0) || y > line(r.len()) {
        let k = if y > line(r.len()) { r.len() } else { k };
        let right = placed.iter().map(|p| p.x + p.w).fold(g.pad + 80.0, f32::max);
        return Target { row: k, col: 0, caret: (g.pad - 4.0, line(k) - 1.5, right - g.pad + 8.0, 3.0), new_row: true };
    }
    let row = (((y - line(0)) / step).floor().max(0.0) as usize).min(r.len() - 1);
    let others: Vec<&Placed> = r[row].iter().filter(|&&i| i != dragged).filter_map(|&i| placed.iter().find(|p| p.idx == i)).collect();
    let col = others.iter().filter(|p| p.x + p.w / 2.0 < x).count();
    let caret_x = match col {
        0 => others.first().map_or(g.pad, |p| p.x) - g.gap / 2.0 - 1.5,
        n => others[n - 1].x + others[n - 1].w + g.gap / 2.0 - 1.5,
    };
    Target { row, col, caret: (caret_x, g.row_y(row), 3.0, g.chip_h), new_row: false }
}

/// Moves item `dragged` to where `target` said (rows counted as displayed, before the move).
pub fn apply_move(items: &mut Vec<Item>, dragged: usize, t: &Target) {
    let (row, col) = (t.row, t.col);
    let item = items[dragged].clone();
    let mut grid: Vec<Vec<Item>> =
        rows(items).iter().map(|r| r.iter().filter(|&&i| i != dragged).map(|&i| items[i].clone()).collect()).collect();
    if t.new_row || row >= grid.len() {
        grid.insert(row.min(grid.len()), vec![item]);
    } else {
        let c = col.min(grid[row].len());
        grid[row].insert(c, item);
    }
    grid.retain(|r| !r.is_empty());
    *items = grid.into_iter().flat_map(|r| r.into_iter().enumerate().map(|(i, it)| Item { newline: i == 0, ..it })).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::layout_default;

    fn ids(items: &[Item]) -> Vec<(&str, bool)> {
        items.iter().map(|i| (i.id.as_str(), i.newline)).collect()
    }

    fn mv(items: &mut Vec<Item>, dragged: usize, row: usize, col: usize, new_row: bool) {
        apply_move(items, dragged, &Target { row, col, caret: (0.0, 0.0, 0.0, 0.0), new_row });
    }

    #[test]
    fn move_between_rows_and_to_a_new_row() {
        let mut v = layout_default(); // rows: [exp_hr pct_hr] [exp_window eta] [level pct exp_now] [gained elapsed]
                                      // eta (3) to the front of row 0
        mv(&mut v, 3, 0, 0, false);
        assert_eq!(&ids(&v)[..3], &[("eta", true), ("exp_hr", false), ("pct_hr", false)]);
        assert_eq!(ids(&v)[3], ("exp_window", true));
        // exp_window is now alone on its row; dragging it to a new row at the end keeps it alone, and the old row is gone
        let at = v.iter().position(|i| i.id == "exp_window").unwrap();
        let rows_before = rows(&v).len();
        mv(&mut v, at, rows_before, 0, true);
        assert_eq!(rows(&v).len(), rows_before);
        assert!(v.last().unwrap().id == "exp_window" && v.last().unwrap().newline);
        assert!(v.iter().filter(|i| i.id == "exp_window").count() == 1 && v.len() == 12);
        assert!(v[0].newline);
    }

    #[test]
    fn hit_and_target() {
        let v = layout_default();
        let g = Geo::new(10);
        let (placed, h) = place(&v, &g);
        assert!(h > g.row_y(4));
        let first = placed[0];
        assert_eq!(hit(&placed, first.x + 2.0, first.y + 2.0), Some(0));
        assert_eq!(hit(&placed, 1.0, 1.0), None);
        // dropping far below the last row means a new row
        assert!(target(&v, &placed, &g, 0, 30.0, 9999.0).new_row);
        // dropping in the middle of row 1, left of everything, lands at column 0
        let t = target(&v, &placed, &g, 0, 2.0, g.row_y(1) + g.chip_h / 2.0);
        assert_eq!((t.row, t.col, t.new_row), (1, 0, false));
        // ...on its top edge, or in the gap above it, makes a new row there
        for y in [g.row_y(1) + 2.0, g.row_y(1) - g.gap / 2.0] {
            let t = target(&v, &placed, &g, 0, 30.0, y);
            assert_eq!((t.row, t.new_row), (1, true), "y {y}");
        }
        // ...and above the first row, a new first row
        let t = target(&v, &placed, &g, 0, 30.0, 2.0);
        assert_eq!((t.row, t.new_row), (0, true));
    }

    #[test]
    fn fill_pieces_take_what_their_row_leaves() {
        // widest row: 100 + 10 + 50 = 160
        let rows = vec![
            vec![(100.0, false), (50.0, false)],
            vec![(40.0, true)],
            vec![(30.0, true), (20.0, false), (10.0, true)],
            vec![(60.0, false)],
        ];
        let w = stretch(&rows, 10.0);
        assert_eq!(w[0], [100.0, 50.0]);
        assert_eq!(w[1], [160.0]); // alone: the whole width
        assert_eq!(w[2], [70.0, 20.0, 50.0]); // 160 - (60 + 2 gaps of 10) = 80 more, half each
        assert_eq!(w[3], [60.0]); // no fill: as it was
                                  // in the editor, a filling chip on its own row is as wide as the widest row
        let mut v = layout_default();
        let spark = v.iter().position(|i| i.id == "spark").unwrap();
        mv(&mut v, spark, 0, 0, true);
        let g = Geo::new(10);
        let (placed, _) = place(&v, &g);
        let right = |p: &Placed| p.x + p.w;
        let widest = placed.iter().map(right).fold(0.0, f32::max);
        assert!((right(placed.iter().find(|p| v[p.idx].id == "spark").unwrap()) - widest).abs() < 0.01);
    }

    #[test]
    fn a_new_row_above_the_others() {
        let mut v = layout_default();
        let spark = v.iter().position(|i| i.id == "spark").unwrap();
        let rows_before = rows(&v).len();
        mv(&mut v, spark, 0, 0, true);
        assert_eq!(ids(&v)[..2], [("spark", true), ("exp_hr", true)]);
        assert_eq!(rows(&v).len(), rows_before + 1);
        // and between rows 1 and 2: the old first row (exp_hr) gets a row of its own under the graph
        let eta = v.iter().position(|i| i.id == "eta").unwrap();
        mv(&mut v, eta, 2, 0, true);
        let r = rows(&v);
        assert_eq!(r[2].iter().map(|&i| v[i].id.as_str()).collect::<Vec<_>>(), ["eta"]);
    }
}
