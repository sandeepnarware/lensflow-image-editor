//! Which photos are selected. The active photo (the one Develop opens and the filmstrip
//! outlines) is the last of `current`.

/// A direction for the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Where a shift-click range starts.
    pub anchor: Option<i64>,
    /// The selected photo ids; the last is the active photo.
    pub current: Vec<i64>,
}

impl Selection {
    pub fn active(&self) -> Option<i64> {
        self.current.last().copied()
    }

    /// Makes `id` the active photo, keeping the selection (or selecting only it when it was
    /// not selected).
    pub fn set_active(&mut self, id: i64) {
        match self.current.iter().position(|c| *c == id) {
            Some(at) => {
                self.current.remove(at);
                self.current.push(id);
            }
            None => self.click(id),
        }
    }

    /// Selects only `id`; it becomes the anchor.
    pub fn click(&mut self, id: i64) {
        self.anchor = Some(id);
        self.current = vec![id];
    }

    /// Selects the photos from the anchor to `id` in `ids_in_order` (either direction), with
    /// `id` active. Without an anchor in the list, a click.
    pub fn shift_click(&mut self, ids_in_order: &[i64], id: i64) {
        let from = self
            .anchor
            .and_then(|a| ids_in_order.iter().position(|&x| x == a));
        let (Some(from), Some(to)) = (from, ids_in_order.iter().position(|&x| x == id)) else {
            self.click(id);
            return;
        };
        self.current = if from <= to {
            ids_in_order[from..=to].to_vec()
        } else {
            ids_in_order[to..=from].iter().rev().copied().collect()
        };
    }

    /// Adds `id` (active, and the anchor) or takes it out of the selection.
    pub fn ctrl_click(&mut self, id: i64) {
        if let Some(at) = self.current.iter().position(|&x| x == id) {
            self.current.remove(at);
        } else {
            self.current.push(id);
            self.anchor = Some(id);
        }
    }

    /// Selects exactly `ids`, in the order given: the last is active and the first becomes
    /// the anchor. The grid's selection box uses this.
    pub fn select(&mut self, ids: &[i64]) {
        self.current = ids.to_vec();
        self.anchor = ids.first().copied();
    }

    /// Adds `ids` to the selection (a Ctrl+drag over the grid), keeping the ones already
    /// selected and their order; the last added is active.
    pub fn add_all(&mut self, ids: &[i64]) {
        for &id in ids {
            if !self.current.contains(&id) {
                self.current.push(id);
            }
        }
        if let Some(last) = ids.last() {
            self.anchor = Some(*last);
        }
    }

    /// Moves the active photo one step (`Left`/`Right`) or one row of `columns` (`Up`/`Down`)
    /// through `order`, clamped to the list, and selects only it. With no active photo in
    /// `order`, the first photo.
    pub fn arrow(&mut self, dir: Dir, columns: usize, order: &[i64]) {
        if order.is_empty() {
            return;
        }
        let at = self
            .active()
            .and_then(|a| order.iter().position(|&x| x == a));
        let Some(at) = at else {
            self.click(order[0]);
            return;
        };
        let last = order.len() - 1;
        let columns = columns.max(1);
        let to = match dir {
            Dir::Left => at.saturating_sub(1),
            Dir::Right => (at + 1).min(last),
            Dir::Up => at.checked_sub(columns).unwrap_or(at),
            Dir::Down => Some(at + columns).filter(|&i| i <= last).unwrap_or(at),
        };
        self.click(order[to]);
    }

    /// Selects every photo in `order`, keeping the active photo active.
    pub fn select_all(&mut self, order: &[i64]) {
        let active = self.active().filter(|a| order.contains(a));
        let mut all: Vec<i64> = order
            .iter()
            .copied()
            .filter(|&x| Some(x) != active)
            .collect();
        all.extend(active);
        if self.anchor.is_none() {
            self.anchor = order.first().copied();
        }
        self.current = all;
    }

    /// Drops ids that are no longer in `order` (after a refresh or a removed folder).
    pub fn retain(&mut self, order: &[i64]) {
        let present: std::collections::HashSet<i64> = order.iter().copied().collect();
        self.current.retain(|id| present.contains(id));
        if self.anchor.is_some_and(|a| !present.contains(&a)) {
            self.anchor = self.active();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [i64; 6] = [10, 11, 12, 13, 14, 15];

    #[test]
    fn shift_click_selects_range_both_directions() {
        let mut s = Selection::default();
        s.click(11);
        s.shift_click(&ORDER, 14);
        assert_eq!(s.current, [11, 12, 13, 14]);
        assert_eq!(s.active(), Some(14));
        // The anchor stays: shift-clicking above it selects upwards instead.
        s.shift_click(&ORDER, 10);
        assert_eq!(s.current, [11, 10]);
        assert_eq!(s.active(), Some(10));
        s.click(15);
        s.shift_click(&ORDER, 12);
        assert_eq!(s.current, [15, 14, 13, 12]);
        assert_eq!(s.anchor, Some(15));
        // With no anchor, a shift-click is a click.
        let mut fresh = Selection::default();
        fresh.shift_click(&ORDER, 13);
        assert_eq!(fresh.current, [13]);
        assert_eq!(fresh.anchor, Some(13));
    }

    #[test]
    fn ctrl_click_toggles() {
        let mut s = Selection::default();
        s.click(10);
        s.ctrl_click(12);
        s.ctrl_click(14);
        assert_eq!(s.current, [10, 12, 14]);
        assert_eq!(s.active(), Some(14));
        s.ctrl_click(12);
        assert_eq!(s.current, [10, 14]);
        s.ctrl_click(14);
        assert_eq!(s.current, [10]);
        assert_eq!(s.active(), Some(10));
        // A shift-click after a ctrl-click ranges from the ctrl-clicked photo.
        s.ctrl_click(13);
        s.shift_click(&ORDER, 15);
        assert_eq!(s.current, [13, 14, 15]);
        s.click(11);
        assert_eq!(s.current, [11]);
    }

    #[test]
    fn arrow_moves_by_columns_and_clamps() {
        // Two rows of three: 10 11 12 / 13 14 15.
        let mut s = Selection::default();
        s.arrow(Dir::Right, 3, &ORDER);
        assert_eq!(s.current, [10], "with nothing selected, the first photo");
        s.arrow(Dir::Down, 3, &ORDER);
        assert_eq!(s.current, [13]);
        s.arrow(Dir::Right, 3, &ORDER);
        assert_eq!(s.current, [14]);
        s.arrow(Dir::Up, 3, &ORDER);
        assert_eq!(s.current, [11]);
        s.arrow(Dir::Up, 3, &ORDER);
        assert_eq!(s.current, [11], "clamped at the top row");
        s.arrow(Dir::Left, 3, &ORDER);
        s.arrow(Dir::Left, 3, &ORDER);
        assert_eq!(s.current, [10], "clamped at the first photo");
        s.click(14);
        s.arrow(Dir::Down, 3, &ORDER);
        assert_eq!(s.current, [14], "no row below");
        s.arrow(Dir::Right, 3, &ORDER);
        s.arrow(Dir::Right, 3, &ORDER);
        assert_eq!(s.current, [15], "clamped at the last photo");
        assert_eq!(s.anchor, Some(15), "the arrow moves the anchor too");
        // A multiple selection collapses to the moved active photo.
        s.click(10);
        s.shift_click(&ORDER, 12);
        s.arrow(Dir::Down, 3, &ORDER);
        assert_eq!(s.current, [15]);
        // An empty folder selects nothing.
        let mut empty = Selection::default();
        empty.arrow(Dir::Down, 3, &[]);
        assert!(empty.current.is_empty());
    }

    #[test]
    fn select_many_replaces_and_add_all_keeps_the_rest() {
        let mut s = Selection::default();
        s.select(&[12, 13, 14]);
        assert_eq!(s.current, [12, 13, 14]);
        assert_eq!(s.active(), Some(14));
        assert_eq!(s.anchor, Some(12));

        s.add_all(&[20, 13]);
        assert_eq!(s.current, [12, 13, 14, 20], "13 was already there");
        assert_eq!(s.active(), Some(20));
        assert_eq!(s.anchor, Some(13), "the last added becomes the anchor");

        s.add_all(&[]);
        assert_eq!(s.active(), Some(20), "nothing added changes nothing");

        s.select(&[]);
        assert!(s.current.is_empty());
        assert_eq!(s.active(), None);
        assert_eq!(s.anchor, None);
    }

    #[test]
    fn select_all_keeps_the_active_photo_active() {
        let mut s = Selection::default();
        s.click(12);
        s.select_all(&ORDER);
        assert_eq!(s.current.len(), 6);
        assert_eq!(s.active(), Some(12));
        let mut none = Selection::default();
        none.select_all(&ORDER);
        assert_eq!(none.current, ORDER);
    }
}
