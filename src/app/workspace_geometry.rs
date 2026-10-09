//! Release-time snapping of outer editor-window rectangles.
//!
//! This module owns no egui interaction state. Native windows keep their pointer gesture;
//! callers may apply the returned rectangle once, after release.
use egui::{Pos2, Rect, Vec2};

pub(super) const SNAP_DISTANCE: f32 = 8.0;

/// The edges owned by a native resize gesture. A corner owns one horizontal and one
/// vertical edge; opposite edges should not both be set on the same axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ResizeEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindowGesture {
    Move,
    Resize(ResizeEdges),
}

#[derive(Clone, Copy)]
struct Candidate {
    delta: f32,
    workspace: bool,
    target: f32,
}

impl Candidate {
    fn precedes(self, other: Self) -> bool {
        self.delta
            .abs()
            .total_cmp(&other.delta.abs())
            .then_with(|| other.workspace.cmp(&self.workspace))
            .then_with(|| self.target.total_cmp(&other.target))
            .then_with(|| self.delta.total_cmp(&other.delta))
            .is_lt()
    }
}

fn overlaps(a_min: f32, a_max: f32, b_min: f32, b_max: f32) -> bool {
    a_min.max(b_min) < a_max.min(b_max)
}

fn consider(
    best: &mut Option<Candidate>,
    edge: f32,
    target: f32,
    workspace: bool,
    minimum_delta: f32,
    maximum_delta: f32,
) {
    let delta = target - edge;
    if !delta.is_finite()
        || delta.abs() > SNAP_DISTANCE
        || delta < minimum_delta
        || delta > maximum_delta
    {
        return;
    }
    let candidate = Candidate {
        delta,
        workspace,
        target,
    };
    if best.is_none_or(|other| candidate.precedes(other)) {
        *best = Some(candidate);
    }
}

/// Normalize one axis while keeping the opposite edge of a resize anchored whenever
/// it is still valid in the workspace. Containment wins if the workspace became smaller.
fn bounded_axis(
    low: f32,
    high: f32,
    bound_low: f32,
    bound_high: f32,
    minimum: f32,
    changed_low: bool,
    changed_high: bool,
) -> (f32, f32) {
    let minimum = minimum.max(0.0).min(bound_high - bound_low);
    if changed_low && !changed_high {
        let high = high.clamp(bound_low + minimum, bound_high);
        (low.clamp(bound_low, high - minimum), high)
    } else if changed_high && !changed_low {
        let low = low.clamp(bound_low, bound_high - minimum);
        (low, high.clamp(low + minimum, bound_high))
    } else {
        let size = (high - low).max(minimum).min(bound_high - bound_low);
        let low = low.clamp(bound_low, bound_high - size);
        (low, low + size)
    }
}

/// Snap a released gesture to workspace edges or the supplied visible peer windows.
///
/// All rectangles and sizes use logical points and refer to the complete outer window.
/// `other_rects` must exclude the moving/resizing window. Peers need positive overlap on
/// the perpendicular axis; merely touching corners does not attract a window. The closest
/// candidate within eight points wins, with workspace edges winning exact distance ties.
/// Remaining ties are independent of peer iteration order.
///
/// A move preserves size unless a now-smaller workspace requires shrinking an oversized
/// window to fit. A resize respects `minimum_size`, capped by workspace size, and preserves
/// the opposite edge whenever that edge remains within bounds. Invalid/non-positive bounds
/// or non-finite input rectangles are returned unchanged; callers must not render these.
/// Empty resize edges perform only minimum-size/containment recovery, without snapping.
pub(super) fn snap_rect(
    rect: Rect,
    bounds: Rect,
    minimum_size: Vec2,
    other_rects: &[Rect],
    gesture: WindowGesture,
) -> Rect {
    if !rect.is_finite() || !bounds.is_finite() || !bounds.is_positive() {
        return rect;
    }
    let edges = match gesture {
        WindowGesture::Move => ResizeEdges::default(),
        WindowGesture::Resize(edges) => edges,
    };
    // Minimum size is already enforced by native windows for moves. Do not make a
    // position-only operation resize a window, even if the requested minimum changed.
    let minimum = match gesture {
        WindowGesture::Move => Vec2::ZERO,
        WindowGesture::Resize(_) => Vec2::new(
            if minimum_size.x.is_finite() {
                minimum_size.x.max(0.0)
            } else {
                0.0
            },
            if minimum_size.y.is_finite() {
                minimum_size.y.max(0.0)
            } else {
                0.0
            },
        )
        .min(bounds.size()),
    };
    let (left, right) = bounded_axis(
        rect.left(),
        rect.right(),
        bounds.left(),
        bounds.right(),
        minimum.x,
        edges.left,
        edges.right,
    );
    let (top, bottom) = bounded_axis(
        rect.top(),
        rect.bottom(),
        bounds.top(),
        bounds.bottom(),
        minimum.y,
        edges.top,
        edges.bottom,
    );
    let bounded = Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom));
    let mut result = bounded;

    for axis in 0..2 {
        let low = bounded.min[axis];
        let high = bounded.max[axis];
        let bound_low = bounds.min[axis];
        let bound_high = bounds.max[axis];
        let changed_low = if axis == 0 { edges.left } else { edges.top };
        let changed_high = if axis == 0 { edges.right } else { edges.bottom };
        let moving = gesture == WindowGesture::Move;
        // Native gestures cannot resize both opposing edges. Treat that malformed
        // descriptor conservatively as recovery-only on this axis.
        if !moving && changed_low == changed_high {
            continue;
        }
        let (minimum_delta, maximum_delta) = if moving {
            (bound_low - low, bound_high - high)
        } else if changed_low {
            (bound_low - low, high - minimum[axis] - low)
        } else {
            (low + minimum[axis] - high, bound_high - high)
        };
        let mut best = None;
        for edge in [low, high].into_iter().enumerate().filter_map(|(i, edge)| {
            (moving || (i == 0 && changed_low) || (i == 1 && changed_high)).then_some(edge)
        }) {
            for target in [bound_low, bound_high] {
                consider(&mut best, edge, target, true, minimum_delta, maximum_delta);
            }
            let perpendicular = 1 - axis;
            for other in other_rects.iter().filter(|other| {
                other.is_finite()
                    && other.is_positive()
                    && overlaps(
                        bounded.min[perpendicular],
                        bounded.max[perpendicular],
                        other.min[perpendicular],
                        other.max[perpendicular],
                    )
            }) {
                for target in [other.min[axis], other.max[axis]] {
                    consider(&mut best, edge, target, false, minimum_delta, maximum_delta);
                }
            }
        }
        if let Some(candidate) = best {
            if moving {
                result.min[axis] += candidate.delta;
                result.max[axis] += candidate.delta;
            } else if changed_low {
                result.min[axis] = candidate.target;
            } else {
                result.max[axis] = candidate.target;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    fn bounds() -> Rect {
        rect(0.0, 0.0, 1000.0, 800.0)
    }

    fn snap(input: Rect, peers: &[Rect], gesture: WindowGesture) -> Rect {
        snap_rect(input, bounds(), Vec2::new(40.0, 30.0), peers, gesture)
    }

    #[test]
    fn moves_snap_to_each_workspace_side_without_resizing() {
        for (input, expected) in [
            (rect(6.0, 120.0, 100.0, 80.0), rect(0.0, 120.0, 100.0, 80.0)),
            (
                rect(894.0, 120.0, 100.0, 80.0),
                rect(900.0, 120.0, 100.0, 80.0),
            ),
            (rect(120.0, 6.0, 100.0, 80.0), rect(120.0, 0.0, 100.0, 80.0)),
            (
                rect(120.0, 714.0, 100.0, 80.0),
                rect(120.0, 720.0, 100.0, 80.0),
            ),
        ] {
            let actual = snap(input, &[], WindowGesture::Move);
            assert_eq!(actual, expected);
            assert_eq!(actual.size(), input.size());
        }
    }

    #[test]
    fn proximity_is_eight_points_inclusive_and_allows_leaving_snap() {
        assert_eq!(
            snap(rect(8.0, 100.0, 100.0, 80.0), &[], WindowGesture::Move).left(),
            0.0
        );
        let outside = rect(8.25, 100.0, 100.0, 80.0);
        assert_eq!(snap(outside, &[], WindowGesture::Move), outside);
        let away = rect(20.0, 100.0, 100.0, 80.0);
        assert_eq!(snap(away, &[], WindowGesture::Move), away);
    }

    #[test]
    fn moves_snap_to_adjacent_and_matching_peer_edges() {
        let peer = rect(300.0, 100.0, 200.0, 200.0);
        assert_eq!(
            snap(
                rect(194.0, 140.0, 100.0, 80.0),
                &[peer],
                WindowGesture::Move
            ),
            rect(200.0, 140.0, 100.0, 80.0),
        );
        assert_eq!(
            snap(
                rect(306.0, 140.0, 100.0, 80.0),
                &[peer],
                WindowGesture::Move
            ),
            rect(300.0, 140.0, 100.0, 80.0),
        );
        assert_eq!(
            snap(
                rect(340.0, 306.0, 100.0, 80.0),
                &[peer],
                WindowGesture::Move
            ),
            rect(340.0, 300.0, 100.0, 80.0),
        );
    }

    #[test]
    fn peers_require_positive_perpendicular_overlap() {
        let peer = rect(300.0, 100.0, 200.0, 200.0);
        for y in [300.0, 320.0] {
            let input = rect(194.0, y, 100.0, 80.0);
            assert_eq!(snap(input, &[peer], WindowGesture::Move), input);
        }
        let input = rect(140.0, 194.0, 80.0, 100.0);
        let peer = rect(220.0, 300.0, 200.0, 200.0);
        assert_eq!(snap(input, &[peer], WindowGesture::Move), input);
    }

    #[test]
    fn closest_candidate_wins_and_workspace_wins_equal_distance() {
        let input = rect(6.0, 120.0, 100.0, 80.0);
        let equally_close = rect(12.0, 100.0, 40.0, 200.0);
        assert_eq!(
            snap(input, &[equally_close], WindowGesture::Move).left(),
            0.0
        );
        let closer = rect(10.0, 100.0, 40.0, 200.0);
        assert_eq!(snap(input, &[closer], WindowGesture::Move).left(), 10.0);
    }

    #[test]
    fn peer_order_does_not_change_tie_result() {
        let input = rect(200.0, 140.0, 100.0, 80.0);
        let a = rect(306.0, 100.0, 50.0, 200.0);
        let b = rect(144.0, 100.0, 50.0, 200.0);
        let forward = snap(input, &[a, b], WindowGesture::Move);
        let backward = snap(input, &[b, a], WindowGesture::Move);
        assert_eq!(forward, backward);
        assert_eq!(forward.left(), 194.0);
    }

    #[test]
    fn all_resize_sides_snap_without_moving_opposite_edges() {
        for (input, edges, expected) in [
            (
                rect(6.0, 100.0, 94.0, 80.0),
                ResizeEdges {
                    left: true,
                    ..Default::default()
                },
                rect(0.0, 100.0, 100.0, 80.0),
            ),
            (
                rect(800.0, 100.0, 194.0, 80.0),
                ResizeEdges {
                    right: true,
                    ..Default::default()
                },
                rect(800.0, 100.0, 200.0, 80.0),
            ),
            (
                rect(100.0, 6.0, 100.0, 94.0),
                ResizeEdges {
                    top: true,
                    ..Default::default()
                },
                rect(100.0, 0.0, 100.0, 100.0),
            ),
            (
                rect(100.0, 600.0, 100.0, 194.0),
                ResizeEdges {
                    bottom: true,
                    ..Default::default()
                },
                rect(100.0, 600.0, 100.0, 200.0),
            ),
        ] {
            assert_eq!(snap(input, &[], WindowGesture::Resize(edges)), expected);
        }
    }

    #[test]
    fn corners_snap_both_active_edges_only() {
        let input = rect(6.0, 6.0, 94.0, 94.0);
        let edges = ResizeEdges {
            left: true,
            top: true,
            ..Default::default()
        };
        assert_eq!(
            snap(input, &[], WindowGesture::Resize(edges)),
            rect(0.0, 0.0, 100.0, 100.0)
        );
        let right_only = ResizeEdges {
            right: true,
            ..Default::default()
        };
        let input = rect(6.0, 6.0, 988.0, 80.0);
        assert_eq!(
            snap(input, &[], WindowGesture::Resize(right_only)),
            rect(6.0, 6.0, 994.0, 80.0)
        );
    }

    #[test]
    fn resize_rejects_attractive_edge_below_minimum() {
        let input = rect(100.0, 140.0, 44.0, 80.0);
        let peer = rect(138.0, 100.0, 100.0, 200.0);
        let edges = ResizeEdges {
            right: true,
            ..Default::default()
        };
        assert_eq!(snap(input, &[peer], WindowGesture::Resize(edges)), input);
        let input = rect(136.0, 140.0, 44.0, 80.0);
        let peer = rect(142.0, 100.0, 100.0, 200.0);
        let edges = ResizeEdges {
            left: true,
            ..Default::default()
        };
        assert_eq!(snap(input, &[peer], WindowGesture::Resize(edges)), input);
    }

    #[test]
    fn minimum_recovery_keeps_the_opposite_resize_edge_anchored() {
        let input = rect(170.0, 140.0, 10.0, 80.0);
        let left = ResizeEdges {
            left: true,
            ..Default::default()
        };
        assert_eq!(
            snap(input, &[], WindowGesture::Resize(left)),
            rect(140.0, 140.0, 40.0, 80.0)
        );
        let right = ResizeEdges {
            right: true,
            ..Default::default()
        };
        assert_eq!(
            snap(input, &[], WindowGesture::Resize(right)),
            rect(170.0, 140.0, 40.0, 80.0)
        );
    }

    #[test]
    fn snap_rejects_peer_alignment_outside_workspace() {
        let input = rect(6.0, 140.0, 100.0, 80.0);
        let outside = rect(-1.0, 100.0, 40.0, 200.0);
        assert_eq!(snap(input, &[outside], WindowGesture::Move).left(), 0.0);
    }

    #[test]
    fn offscreen_and_oversized_rects_recover_inside_small_workspace() {
        let bounds = rect(180.0, 115.0, 32.0, 20.0);
        let input = rect(-4000.0, 9000.0, 9000.0, 4000.0);
        for gesture in [
            WindowGesture::Move,
            WindowGesture::Resize(ResizeEdges::default()),
        ] {
            assert_eq!(
                snap_rect(input, bounds, Vec2::new(440.0, 410.0), &[], gesture),
                bounds
            );
        }
    }

    #[test]
    fn move_does_not_apply_a_new_minimum_size() {
        let input = rect(100.0, 100.0, 20.0, 15.0);
        assert_eq!(snap(input, &[], WindowGesture::Move), input);
    }

    #[test]
    fn snapping_is_idempotent_and_does_not_accumulate_size_drift() {
        let input = rect(194.0, 140.0, 100.0, 80.0);
        let peers = [rect(300.0, 100.0, 200.0, 200.0)];
        let expected = snap(input, &peers, WindowGesture::Move);
        let mut actual = input;
        for _ in 0..100 {
            actual = snap(actual, &peers, WindowGesture::Move);
            assert_eq!(actual, expected);
            assert_eq!(actual.size(), input.size());
        }
    }

    #[test]
    fn malformed_peers_are_ignored() {
        let input = rect(100.0, 100.0, 100.0, 80.0);
        let peers = [
            Rect::NOTHING,
            rect(f32::NAN, 0.0, 1.0, 1.0),
            rect(200.0, 100.0, -1.0, 10.0),
        ];
        assert_eq!(snap(input, &peers, WindowGesture::Move), input);
    }
}
