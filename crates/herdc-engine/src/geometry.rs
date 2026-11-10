//! Where each pane of a tab sits: what `pane.go` and a directional
//! `pane.swap` ask. The desktop measures its window; here the tree's own
//! proportions are laid out on a square (`px` sizes as if it were 1000 px).

use herdc_model::layout::{Kind, Node, Size};

pub const SIDE: f32 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const SQUARE: Rect = Rect { x: 0.0, y: 0.0, w: SIDE, h: SIDE };

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Every visible block of `root` (hidden nodes and inactive stack children
/// skipped), with its place in `area`.
pub fn rects(root: &Node, area: Rect) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    place(root, area, &mut out);
    out
}

fn place(n: &Node, area: Rect, out: &mut Vec<(String, Rect)>) {
    if n.hidden {
        return;
    }
    match &n.kind {
        Kind::Block { .. } => out.push((n.id.clone(), area)),
        Kind::Stack { .. } => {
            if let Some(child) = n.active_child() {
                place(child, area, out);
            }
        }
        Kind::Row { .. } | Kind::Column { .. } => {
            let mut placed = Vec::new();
            place_children(n, area, &mut placed);
            for (c, r) in placed {
                place(c, r, out);
            }
        }
    }
}

/// Where two panes (or splits) side by side meet: dragged, it shares what
/// the two have between them anew. Only between two sized by weight (a
/// `px` one keeps its size).
#[derive(Clone, Debug, PartialEq)]
pub struct Seam {
    /// The nodes before and after it.
    pub before: String,
    pub after: String,
    /// Side by side (a row: the seam is upright), or one above the other.
    pub row: bool,
    /// From the start of `before` to the end of `after`, along the split.
    pub from: f32,
    pub to: f32,
    /// Where they meet now.
    pub at: f32,
    /// Its extent across: start and length.
    pub across: (f32, f32),
    /// The two weights together.
    pub weight: f32,
}

impl Seam {
    /// The weights of `before` and `after` with the seam moved to `at`;
    /// neither less than a tenth of the two.
    pub fn weights(&self, at: f32) -> (f32, f32) {
        let span = (self.to - self.from).max(1.0);
        let share = ((at - self.from) / span).clamp(0.1, 0.9);
        (self.weight * share, self.weight * (1.0 - share))
    }
}

/// The seams of `root`'s visible splits, laid out in `area`.
pub fn seams(root: &Node, area: Rect) -> Vec<Seam> {
    let mut out = Vec::new();
    seams_in(root, area, &mut out);
    out
}

fn seams_in(n: &Node, area: Rect, out: &mut Vec<Seam>) {
    if n.hidden {
        return;
    }
    match &n.kind {
        Kind::Block { .. } => {}
        Kind::Stack { .. } => {
            if let Some(child) = n.active_child() {
                seams_in(child, area, out);
            }
        }
        Kind::Row { .. } | Kind::Column { .. } => {
            let row = matches!(n.kind, Kind::Row { .. });
            // The children's places, as `place` makes them.
            let mut placed = Vec::new();
            place_children(n, area, &mut placed);
            for pair in placed.windows(2) {
                let ((a, ra), (b, rb)) = (&pair[0], &pair[1]);
                if px(a).is_none() && px(b).is_none() {
                    let (from, at, to) = if row { (ra.x, rb.x, rb.right()) } else { (ra.y, rb.y, rb.bottom()) };
                    let across = if row { (area.y, area.h) } else { (area.x, area.w) };
                    out.push(Seam { before: a.id.clone(), after: b.id.clone(), row, from, to, at, across, weight: weight(a) + weight(b) });
                }
            }
            for (c, r) in placed {
                seams_in(c, r, out);
            }
        }
    }
}

/// A split's visible children with their places in `area`.
fn place_children<'a>(n: &'a Node, area: Rect, out: &mut Vec<(&'a Node, Rect)>) {
    let (Kind::Row { children } | Kind::Column { children }) = &n.kind else { return };
    let row = matches!(n.kind, Kind::Row { .. });
    let shown: Vec<&Node> = children.iter().filter(|c| !c.hidden).collect();
    let total = if row { area.w } else { area.h };
    let fixed: f32 = shown.iter().filter_map(|c| px(c)).sum();
    let weights: f32 = shown.iter().filter(|c| px(c).is_none()).map(|c| weight(c)).sum();
    let free = (total - fixed).max(0.0);
    let mut at = if row { area.x } else { area.y };
    for c in shown {
        let span = px(c).unwrap_or_else(|| if weights > 0.0 { free * weight(c) / weights } else { 0.0 });
        out.push((c, if row { Rect { x: at, w: span, ..area } } else { Rect { y: at, h: span, ..area } }));
        at += span;
    }
}

fn px(n: &Node) -> Option<f32> {
    match n.size {
        Some(Size::Px { px }) => Some(px),
        _ => None,
    }
}

fn weight(n: &Node) -> f32 {
    match n.size {
        Some(Size::Weight { weight }) => weight.max(0.0),
        _ => 1.0,
    }
}

/// The nearest block from `cur` in `dir`, preferring the most recently
/// focused along that edge; unvisited ones by overlap and layout order.
///
/// From herdr-app `src/window.rs` (`nearest`), Apache-2.0.
pub fn nearest(cur: Rect, dir: &str, candidates: impl IntoIterator<Item = (String, Rect)>, history: &[String]) -> Option<String> {
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
    let cmp = |a: f32, b: f32| if (a - b).abs() < 1.0 { std::cmp::Ordering::Equal } else { a.total_cmp(&b) };
    candidates
        .into_iter()
        .filter_map(|(id, r)| {
            let (gap, ov) = match dir {
                "left" => (cur.x - r.right(), overlap(cur.y, cur.bottom(), r.y, r.bottom())),
                "right" => (r.x - cur.right(), overlap(cur.y, cur.bottom(), r.y, r.bottom())),
                "up" => (cur.y - r.bottom(), overlap(cur.x, cur.right(), r.x, r.right())),
                _ => (r.y - cur.bottom(), overlap(cur.x, cur.right(), r.x, r.right())),
            };
            let recent = history.iter().position(|pane| pane == &id);
            (gap > -2.0 && ov > 0.0).then_some((gap, recent, ov, id))
        })
        .reduce(|best, next| if cmp(next.0, best.0).then(best.1.cmp(&next.1)).then(cmp(best.2, next.2)).is_lt() { next } else { best })
        .map(|(_, _, _, id)| id)
}

/// Keeps the last focused at the end, once.
pub fn remember(history: &mut Vec<String>, id: &str) {
    if history.last().is_some_and(|last| last == id) {
        return;
    }
    history.retain(|pane| pane != id);
    history.push(id.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdc_model::layout::Axis;
    use herdc_model::model::pane;

    fn at<'a>(r: &'a [(String, Rect)], id: &str) -> &'a Rect {
        &r.iter().find(|(i, _)| i == id).unwrap().1
    }

    #[test]
    fn weights_and_px_share_the_square() {
        let mut right = pane("b", "t2");
        right.size = Some(Size::Weight { weight: 3.0 });
        let mut fixed = pane("c", "t3");
        fixed.size = Some(Size::Px { px: 200.0 });
        let col = Node::container("col", Axis::Column, vec![right, fixed]);
        let root = Node::container("row", Axis::Row, vec![pane("a", "t1"), col]);
        let r = rects(&root, Rect::SQUARE);
        assert_eq!(at(&r, "a").w, 500.0);
        assert_eq!(at(&r, "b").x, 500.0);
        assert_eq!((at(&r, "b").h, at(&r, "c").h), (800.0, 200.0));
    }

    #[test]
    fn seams_share_two_weights_anew_and_leave_px_alone() {
        let mut fixed = pane("c", "t3");
        fixed.size = Some(Size::Px { px: 200.0 });
        let col = Node::container("col", Axis::Column, vec![pane("b", "t2"), fixed]);
        let root = Node::container("row", Axis::Row, vec![pane("a", "t1"), col]);
        let s = seams(&root, Rect::SQUARE);
        // Between a and the column; none above the px pane.
        assert_eq!(s.len(), 1, "{s:?}");
        let seam = &s[0];
        assert_eq!((seam.before.as_str(), seam.after.as_str(), seam.row), ("a", "col", true));
        assert_eq!((seam.from, seam.at, seam.to, seam.across), (0.0, 500.0, 1000.0, (0.0, 1000.0)));
        assert_eq!(seam.weights(250.0), (0.5, 1.5));
        // Never past a tenth of either.
        let (a, b) = seam.weights(990.0);
        assert!((a - 1.8).abs() < 1e-4 && (b - 0.2).abs() < 1e-4, "{a} {b}");
    }

    #[test]
    fn going_back_into_a_split_column_takes_the_recent_pane() {
        let col = Node::container("col", Axis::Column, vec![pane("b", "t2"), pane("c", "t3")]);
        let root = Node::container("row", Axis::Row, vec![pane("a", "t1"), col]);
        let r = rects(&root, Rect::SQUARE);
        let others = |from: &str| r.iter().filter(|(i, _)| i != from).cloned().collect::<Vec<_>>();
        assert_eq!(nearest(*at(&r, "a"), "right", others("a"), &[]), Some("b".into()));
        assert_eq!(nearest(*at(&r, "a"), "right", others("a"), &["c".into()]), Some("c".into()));
        assert_eq!(nearest(*at(&r, "b"), "down", others("b"), &[]), Some("c".into()));
        assert_eq!(nearest(*at(&r, "b"), "up", others("b"), &[]), None);
    }
}
