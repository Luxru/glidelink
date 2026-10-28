use serde::{Deserialize, Serialize};

/// Where the peer screen is placed relative to this machine.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

/// A position in the remote screen, normalized to 0..=1.
///
/// Values can temporarily fall outside that range while the pointer is crossing
/// the return edge. Injection clamps them before drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NormPoint {
    pub x: f32,
    pub y: f32,
}

impl NormPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn clamped(self) -> Self {
        Self {
            x: self.x.clamp(0.0, 1.0),
            y: self.y.clamp(0.0, 1.0),
        }
    }
}

/// Keep the injected pointer just inside the screen on entry. This prevents a
/// single noisy delta from immediately triggering the return gesture.
pub const ENTRY_INSET: f32 = 0.006;
/// How far past the remote screen edge the pointer must travel to return.
pub const RETURN_THRESHOLD: f32 = 0.004;
/// Ignore edge re-entry for this long after returning to the host.
pub const REENTRY_COOLDOWN_MS: u64 = 300;
/// Largest peer offset along the shared edge, normalized to the host edge.
/// Keeping some overlap means the pointer always has a route back.
pub const MAX_OFFSET: f32 = 0.75;

/// Convert an absolute pointer position inside the host's virtual desktop to a
/// 0..=1 position. Origins can be negative on multi-monitor setups.
pub fn host_point_to_norm(px: f64, py: f64, x: i32, y: i32, w: i32, h: i32) -> NormPoint {
    let width = (w.max(2) - 1) as f64;
    let height = (h.max(2) - 1) as f64;
    NormPoint::new(
        ((px - x as f64) / width) as f32,
        ((py - y as f64) / height) as f32,
    )
}

/// Map a host pointer position to the point where it enters the peer screen.
///
/// `offset` shifts the peer screen along the shared edge, normalized to the
/// host edge. Positive means the peer is shifted down (left/right) or right
/// (top/bottom). Points that fall outside the peer are clamped to its edge.
pub fn entry_point(edge: Edge, host: NormPoint, offset: f32) -> NormPoint {
    let offset = offset.clamp(-MAX_OFFSET, MAX_OFFSET);
    let point = match edge {
        Edge::Right => NormPoint::new(ENTRY_INSET, host.y - offset),
        Edge::Left => NormPoint::new(1.0 - ENTRY_INSET, host.y - offset),
        Edge::Top => NormPoint::new(host.x - offset, 1.0 - ENTRY_INSET),
        Edge::Bottom => NormPoint::new(host.x - offset, ENTRY_INSET),
    };
    point.clamped()
}

/// Advance the pointer inside the peer screen. Deltas are normalized against
/// the *host* desktop, so a full host-screen traversal maps to a full peer
/// traversal regardless of the two resolutions.
pub fn advance(point: NormPoint, dx_px: f64, dy_px: f64, host_w: i32, host_h: i32) -> NormPoint {
    NormPoint::new(
        point.x + (dx_px / host_w.max(1) as f64) as f32,
        point.y + (dy_px / host_h.max(1) as f64) as f32,
    )
}

/// True when the pointer has crossed the edge that leads back to the host.
pub fn has_returned(edge: Edge, point: NormPoint) -> bool {
    match edge {
        Edge::Right => point.x < -RETURN_THRESHOLD,
        Edge::Left => point.x > 1.0 + RETURN_THRESHOLD,
        Edge::Top => point.y > 1.0 + RETURN_THRESHOLD,
        Edge::Bottom => point.y < -RETURN_THRESHOLD,
    }
}

/// Host position (normalized to the full virtual desktop) to warp to when
/// returning from the peer. The position is kept a couple of pixels inside the
/// screen to avoid instantly re-entering the peer. `offset` must match the
/// value used by [`entry_point`] so the pointer returns at the same height.
pub fn host_return_point(edge: Edge, point: NormPoint, offset: f32) -> NormPoint {
    let offset = offset.clamp(-MAX_OFFSET, MAX_OFFSET);
    let p = point.clamped();
    let host = match edge {
        Edge::Right => NormPoint::new(0.9995, p.y + offset),
        Edge::Left => NormPoint::new(0.0005, p.y + offset),
        Edge::Top => NormPoint::new(p.x + offset, 0.0005),
        Edge::Bottom => NormPoint::new(p.x + offset, 0.9995),
    };
    host.clamped()
}

/// Whether a raw delta points away from the host and into the peer.
pub fn is_outward(edge: Edge, dx: f64, dy: f64) -> bool {
    match edge {
        Edge::Right => dx > 0.0,
        Edge::Left => dx < 0.0,
        Edge::Top => dy < 0.0,
        Edge::Bottom => dy > 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_edge_round_trip() {
        let entry = entry_point(Edge::Right, NormPoint::new(1.0, 0.4), 0.0);
        assert!(entry.x > 0.0 && entry.x < 0.01);
        let moved = advance(entry, 100.0, 0.0, 1920, 1080);
        assert!(moved.x > entry.x);
        assert!(!has_returned(Edge::Right, moved));

        let returned = NormPoint::new(-0.01, 0.4);
        assert!(has_returned(Edge::Right, returned));
        let host = host_return_point(Edge::Right, returned, 0.0);
        assert!(host.x > 0.999);
    }

    #[test]
    fn top_and_bottom_edges() {
        let top = entry_point(Edge::Top, NormPoint::new(0.3, 0.0), 0.0);
        assert!((top.y - (1.0 - ENTRY_INSET)).abs() < f32::EPSILON);
        assert!(has_returned(Edge::Top, NormPoint::new(0.3, 1.1)));

        let bottom = entry_point(Edge::Bottom, NormPoint::new(0.3, 1.0), 0.0);
        assert!((bottom.y - ENTRY_INSET).abs() < f32::EPSILON);
        assert!(has_returned(Edge::Bottom, NormPoint::new(0.3, -0.1)));
    }

    #[test]
    fn peer_offset_shifts_entry_and_return() {
        // Peer moved down: a host point maps above the peer center.
        let entry = entry_point(Edge::Right, NormPoint::new(1.0, 0.6), 0.2);
        assert!((entry.y - 0.4).abs() < 1e-6);
        let host = host_return_point(Edge::Right, NormPoint::new(0.0, 0.4), 0.2);
        assert!((host.y - 0.6).abs() < 1e-6);

        // Peer moved right on a vertical edge.
        let entry = entry_point(Edge::Top, NormPoint::new(0.5, 0.0), 0.25);
        assert!((entry.x - 0.25).abs() < 1e-6);
        let host = host_return_point(Edge::Top, NormPoint::new(0.25, 1.0), 0.25);
        assert!((host.x - 0.5).abs() < 1e-6);
    }

    #[test]
    fn entry_and_return_clamp_to_screen() {
        let entry = entry_point(Edge::Right, NormPoint::new(1.0, 0.05), 0.5);
        assert_eq!(entry.y, 0.0);
        let host = host_return_point(Edge::Right, NormPoint::new(0.0, 0.9), 0.5);
        assert_eq!(host.y, 1.0);

        let entry = entry_point(Edge::Bottom, NormPoint::new(0.9, 1.0), -0.5);
        assert_eq!(entry.x, 1.0);
        let entry = entry_point(Edge::Bottom, NormPoint::new(0.1, 1.0), 0.5);
        assert_eq!(entry.x, 0.0);
    }

    #[test]
    fn outward_direction_is_edge_relative() {
        assert!(is_outward(Edge::Right, 1.0, 0.0));
        assert!(!is_outward(Edge::Right, -1.0, 0.0));
        assert!(is_outward(Edge::Left, -1.0, 0.0));
        assert!(is_outward(Edge::Top, 0.0, -1.0));
        assert!(is_outward(Edge::Bottom, 0.0, 1.0));
    }
}
