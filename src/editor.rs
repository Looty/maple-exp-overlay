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

/// Every chip's rectangle, and the editor's height (with one empty row below, to drop a chip on for a new row).
pub fn place(items: &[Item], g: &Geo) -> (Vec<Placed>, f32) {
    let mut out = Vec::new();
    let r = rows(items);
    for (ri, row) in r.iter().enumerate() {
        let mut x = g.pad;
        for &i in row {
            let w = 22.0 + sample(&items[i].id).chars().count() as f32 * g.font_px * 0.6;
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
    pub row: usize,
    pub col: usize,
    /// The insertion marker: (x, y, height).
    pub caret: (f32, f32, f32),
    pub new_row: bool,
}

pub fn target(items: &[Item], placed: &[Placed], g: &Geo, dragged: usize, x: f32, y: f32) -> Target {
    let r = rows(items);
    let row = (((y - g.pad + g.gap / 2.0) / (g.chip_h + g.gap)).floor().max(0.0) as usize).min(r.len());
    if row >= r.len() {
        return Target { row: r.len(), col: 0, caret: (g.pad - 4.0, g.row_y(r.len()), g.chip_h), new_row: true };
    }
    let others: Vec<&Placed> = r[row].iter().filter(|&&i| i != dragged).filter_map(|&i| placed.iter().find(|p| p.idx == i)).collect();
    let col = others.iter().filter(|p| p.x + p.w / 2.0 < x).count();
    let caret_x = match col {
        0 => others.first().map_or(g.pad, |p| p.x) - g.gap / 2.0 - 1.5,
        n => others[n - 1].x + others[n - 1].w + g.gap / 2.0 - 1.5,
    };
    Target { row, col, caret: (caret_x, g.row_y(row), g.chip_h), new_row: false }
}

/// Moves item `dragged` to (row, col) of the layout `target` computed (rows counted as displayed, before the move).
pub fn apply_move(items: &mut Vec<Item>, dragged: usize, row: usize, col: usize) {
    let item = items[dragged].clone();
    let mut grid: Vec<Vec<Item>> =
        rows(items).iter().map(|r| r.iter().filter(|&&i| i != dragged).map(|&i| items[i].clone()).collect()).collect();
    if row >= grid.len() {
        grid.push(vec![item]);
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

    #[test]
    fn move_between_rows_and_to_a_new_row() {
        let mut v = layout_default(); // rows: [exp_hr pct_hr] [exp_window eta] [level pct exp_now] [gained elapsed]
                                      // eta (3) to the front of row 0
        apply_move(&mut v, 3, 0, 0);
        assert_eq!(&ids(&v)[..3], &[("eta", true), ("exp_hr", false), ("pct_hr", false)]);
        assert_eq!(ids(&v)[3], ("exp_window", true));
        // exp_window is now alone on its row; dragging it to a new row at the end keeps it alone, and the old row is gone
        let at = v.iter().position(|i| i.id == "exp_window").unwrap();
        let rows_before = rows(&v).len();
        apply_move(&mut v, at, rows_before, 0);
        assert_eq!(rows(&v).len(), rows_before);
        assert!(v.last().unwrap().id == "exp_window" && v.last().unwrap().newline);
        assert!(v.iter().filter(|i| i.id == "exp_window").count() == 1 && v.len() == 10);
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
        // dropping in row 1, left of everything, lands at column 0
        let t = target(&v, &placed, &g, 0, 2.0, g.row_y(1) + 3.0);
        assert_eq!((t.row, t.col), (1, 0));
    }
}
