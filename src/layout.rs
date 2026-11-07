//! Tiling layout of the mini windows (file views and terminals) inside one tab:
//! a binary tree of horizontal/vertical splits.
use egui::{pos2, Rect};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    Right,
    Left,
    Down,
    Up,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Layout {
    /// Index into the app's pane list.
    Leaf(usize),
    Split {
        /// `true` = side by side (vertical divider), `false` = stacked.
        horizontal: bool,
        /// Share of the first child (0..1).
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

/// A draggable divider between two children of a split node.
pub struct Handle {
    /// Path from the root (false = a, true = b) to the split node.
    pub path: Vec<bool>,
    pub rect: Rect,
    /// Area covered by the split node (for converting drags into ratios).
    pub area: Rect,
    pub horizontal: bool,
}

impl Layout {
    pub fn leaves(&self) -> Vec<usize> {
        match self {
            Layout::Leaf(i) => vec![*i],
            Layout::Split { a, b, .. } => [a.leaves(), b.leaves()].concat(),
        }
    }

    /// Number of columns and rows of the grid this layout forms.
    pub fn dims(&self) -> (usize, usize) {
        match self {
            Layout::Leaf(_) => (1, 1),
            Layout::Split { horizontal, a, b, .. } => {
                let ((ac, ar), (bc, br)) = (a.dims(), b.dims());
                if *horizontal { (ac + bc, ar.max(br)) } else { (ac.max(bc), ar + br) }
            }
        }
    }

    /// Splits stay within a 2 × 2 grid: at most 4 panes, 2 columns and 2 rows.
    pub fn can_split(&self, target: usize, dir: SplitDir) -> bool {
        self.can_dock(target, &Layout::Leaf(usize::MAX), dir)
    }

    /// Would docking the whole layout `sub` beside `target` keep the 2 × 2 limit?
    pub fn can_dock(&self, target: usize, sub: &Layout, dir: SplitDir) -> bool {
        let mut l = self.clone();
        if !l.dock(target, sub.clone(), dir) {
            return false;
        }
        Self::fits(&l)
    }

    pub fn fits(l: &Layout) -> bool {
        let (c, r) = l.dims();
        c <= 2 && r <= 2 && l.leaves().len() <= 4
    }

    /// Replace the leaf `target` with a split containing it and `new`.
    pub fn split(&mut self, target: usize, new: usize, dir: SplitDir) -> bool {
        self.dock(target, Layout::Leaf(new), dir)
    }

    /// Replace the leaf `target` with a split containing it and the layout `sub`
    /// on the `dir` side.
    pub fn dock(&mut self, target: usize, sub: Layout, dir: SplitDir) -> bool {
        match self {
            Layout::Leaf(i) if *i == target => {
                let (horizontal, new_first) = match dir {
                    SplitDir::Right => (true, false),
                    SplitDir::Left => (true, true),
                    SplitDir::Down => (false, false),
                    SplitDir::Up => (false, true),
                };
                let (a, b) = if new_first { (sub, Layout::Leaf(target)) } else { (Layout::Leaf(target), sub) };
                *self = Layout::Split { horizontal, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { a, b, .. } => {
                if a.leaves().contains(&target) {
                    a.dock(target, sub, dir)
                } else {
                    b.dock(target, sub, dir)
                }
            }
        }
    }

    /// Add `n` to every leaf index (before merging this layout into another).
    pub fn offset(&mut self, n: usize) {
        match self {
            Layout::Leaf(i) => *i += n,
            Layout::Split { a, b, .. } => {
                a.offset(n);
                b.offset(n);
            }
        }
    }

    /// Take leaf `target` out of the tree (its sibling takes its place) without
    /// renumbering the other leaves. False when `target` is the only leaf.
    pub fn detach(&mut self, target: usize) -> bool {
        !matches!(self, Layout::Leaf(_)) && self.remove_inner(target)
    }

    /// Remove leaf `target` (its sibling takes its place) and renumber the
    /// remaining leaves as if `target` was removed from the pane list.
    /// Returns false when `target` is the only pane.
    pub fn remove(&mut self, target: usize) -> bool {
        if !self.detach(target) {
            return false;
        }
        self.renumber(target);
        true
    }

    fn remove_inner(&mut self, target: usize) -> bool {
        let Layout::Split { a, b, .. } = self else { return false };
        if **a == Layout::Leaf(target) {
            *self = (**b).clone();
            return true;
        }
        if **b == Layout::Leaf(target) {
            *self = (**a).clone();
            return true;
        }
        a.remove_inner(target) || b.remove_inner(target)
    }

    pub fn renumber(&mut self, removed: usize) {
        match self {
            Layout::Leaf(i) => {
                if *i > removed {
                    *i -= 1;
                }
            }
            Layout::Split { a, b, .. } => {
                a.renumber(removed);
                b.renumber(removed);
            }
        }
    }

    /// Compute pane rectangles and divider handles inside `area`.
    pub fn arrange(&self, area: Rect, gap: f32, panes: &mut Vec<(usize, Rect)>, handles: &mut Vec<Handle>) {
        self.arrange_inner(area, gap, &mut Vec::new(), panes, handles);
    }

    fn arrange_inner(&self, area: Rect, gap: f32, path: &mut Vec<bool>, panes: &mut Vec<(usize, Rect)>, handles: &mut Vec<Handle>) {
        match self {
            Layout::Leaf(i) => panes.push((*i, area)),
            Layout::Split { horizontal, ratio, a, b } => {
                let (ra, rb, h) = if *horizontal {
                    let x = area.left() + (area.width() - gap) * ratio;
                    (
                        Rect::from_min_max(area.min, pos2(x, area.bottom())),
                        Rect::from_min_max(pos2(x + gap, area.top()), area.max),
                        Rect::from_min_max(pos2(x, area.top()), pos2(x + gap, area.bottom())),
                    )
                } else {
                    let y = area.top() + (area.height() - gap) * ratio;
                    (
                        Rect::from_min_max(area.min, pos2(area.right(), y)),
                        Rect::from_min_max(pos2(area.left(), y + gap), area.max),
                        Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + gap)),
                    )
                };
                handles.push(Handle { path: path.clone(), rect: h, area, horizontal: *horizontal });
                path.push(false);
                a.arrange_inner(ra, gap, path, panes, handles);
                path.pop();
                path.push(true);
                b.arrange_inner(rb, gap, path, panes, handles);
                path.pop();
            }
        }
    }

    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        match (self, path.split_first()) {
            (Layout::Split { ratio, .. }, None) => *ratio = value.clamp(0.12, 0.88),
            (Layout::Split { a, b, .. }, Some((first, rest))) => {
                if *first { b.set_ratio(rest, value) } else { a.set_ratio(rest, value) }
            }
            _ => {}
        }
    }

    /// Next pane after `current` in reading order (for Tab).
    pub fn next_after(&self, current: usize) -> usize {
        let l = self.leaves();
        let pos = l.iter().position(|&i| i == current).unwrap_or(0);
        l[(pos + 1) % l.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn at_most_two_by_two() {
        let mut l = Layout::Leaf(0);
        assert!(l.can_split(0, SplitDir::Right));
        l.split(0, 1, SplitDir::Right); // [0 | 1]
        assert!(!l.can_split(1, SplitDir::Right), "no third column");
        assert!(!l.can_split(0, SplitDir::Left));
        assert!(l.can_split(1, SplitDir::Down));
        l.split(1, 2, SplitDir::Down); // [0 | 1/2]
        assert!(!l.can_split(2, SplitDir::Down), "no third row");
        assert!(l.can_split(0, SplitDir::Up));
        l.split(0, 3, SplitDir::Up); // 2 x 2
        assert_eq!(l.dims(), (2, 2));
        for i in 0..4 {
            for d in [SplitDir::Right, SplitDir::Left, SplitDir::Down, SplitDir::Up] {
                assert!(!l.can_split(i, d), "the grid is full");
            }
        }
        // Stacked first, then side by side, is the same 2 x 2 limit.
        let mut l = Layout::Leaf(0);
        l.split(0, 1, SplitDir::Down);
        assert!(!l.can_split(0, SplitDir::Up));
        assert!(l.can_split(0, SplitDir::Right) && l.can_split(1, SplitDir::Left));
    }

    #[test]
    fn split_close_and_arrange() {
        let mut l = Layout::Leaf(0);
        assert!(!l.remove(0), "the last pane cannot be closed");
        assert!(l.split(0, 1, SplitDir::Right));
        assert!(l.split(1, 2, SplitDir::Down));
        assert!(l.split(0, 3, SplitDir::Up));
        assert_eq!(l.leaves(), vec![3, 0, 1, 2]);

        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let (mut panes, mut handles) = (vec![], vec![]);
        l.arrange(area, 10.0, &mut panes, &mut handles);
        assert_eq!(panes.len(), 4);
        assert_eq!(handles.len(), 3);
        let r1 = panes.iter().find(|p| p.0 == 1).unwrap().1;
        let r2 = panes.iter().find(|p| p.0 == 2).unwrap().1;
        assert!(r1.left() > 500.0 && r1.bottom() < r2.top(), "1 is right-top, 2 below it");

        // Drag the root divider.
        l.set_ratio(&handles[0].path, 0.3);
        let (mut panes, mut handles2) = (vec![], vec![]);
        l.arrange(area, 10.0, &mut panes, &mut handles2);
        assert!((handles2[0].rect.left() - 297.0).abs() < 1.0);

        // Close pane 0: its sibling (3) takes the left column; indices shift down.
        assert!(l.remove(0));
        assert_eq!(l.leaves(), vec![2, 0, 1]);
        assert_eq!(l.next_after(2), 0);
        assert_eq!(l.next_after(1), 2);
    }

    #[test]
    fn dock_detach_and_offset() {
        // [0 | 1]; move 0 below 1 (detach, then dock beside 1).
        let mut l = Layout::Leaf(0);
        l.split(0, 1, SplitDir::Right);
        assert!(l.detach(0));
        assert_eq!(l, Layout::Leaf(1));
        assert!(l.dock(1, Layout::Leaf(0), SplitDir::Down));
        assert_eq!(l.leaves(), vec![1, 0]);
        assert_eq!(l.dims(), (1, 2));
        // A whole two-column tab docked to the right would make three columns.
        let mut other = Layout::Leaf(0);
        other.split(0, 1, SplitDir::Right);
        other.offset(2);
        assert_eq!(other.leaves(), vec![2, 3]);
        let mut cols = Layout::Leaf(0);
        cols.split(0, 1, SplitDir::Right);
        assert!(!cols.can_dock(1, &other, SplitDir::Right), "no third column");
        let stacked = Layout::Leaf(0);
        assert!(stacked.can_dock(0, &other, SplitDir::Down), "a row of two below one window fits");
        let mut s2 = stacked.clone();
        s2.dock(0, other, SplitDir::Down);
        assert_eq!(s2.dims(), (2, 2));
        assert!(!Layout::Leaf(0).detach(0), "the last window stays");
    }

    #[test]
    fn saved_with_the_session() {
        let mut l = Layout::Leaf(0);
        l.split(0, 1, SplitDir::Right);
        l.split(1, 2, SplitDir::Down);
        let tab = crate::config::SavedTab { layout: Some(l.clone()), focus: 2, ..Default::default() };
        let back: crate::config::SavedTab = serde_json::from_str(&serde_json::to_string(&tab).unwrap()).unwrap();
        assert_eq!(back.layout, Some(l));
        // Sessions saved before split tabs existed still load.
        let old: crate::config::SavedTab = serde_json::from_str(r#"{"path":"/tmp","terminal":false,"group":null,"pinned":false,"title":null}"#).unwrap();
        assert!(old.windows.is_empty() && old.layout.is_none());
    }
}
