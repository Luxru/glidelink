use crate::status::ScreenInfo;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

pub type DeviceId = [u8; 16];

pub const MAX_DEVICES: usize = 8;
pub const ENTRY_INSET: f32 = 0.006;
pub const REENTRY_COOLDOWN_MS: u64 = 300;
pub const ADJACENCY_EPSILON: f32 = 0.001;
pub const OVERLAP_EPSILON: f32 = 0.0005;
pub const MIN_OVERLAP_SPAN: f32 = 0.02;

pub fn new_device_id() -> DeviceId {
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut id);
    id
}

pub fn device_id_to_hex(id: &DeviceId) -> String {
    let mut out = String::with_capacity(32);
    for byte in id {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn device_id_from_hex(value: &str) -> Option<DeviceId> {
    let value = value.trim();
    if value.len() != 32 {
        return None;
    }
    let mut id = [0u8; 16];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        id[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(id)
}

/// A position in a screen, normalized to 0..=1.
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

    pub fn inset(self) -> Self {
        Self {
            x: self.x.clamp(ENTRY_INSET, 1.0 - ENTRY_INSET),
            y: self.y.clamp(ENTRY_INSET, 1.0 - ENTRY_INSET),
        }
    }
}

/// A point in the virtual world. One world unit is the host screen width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldPoint {
    pub x: f32,
    pub y: f32,
}

impl WorldPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center(self) -> WorldPoint {
        WorldPoint::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(self, point: WorldPoint) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
    }

    pub fn clamp(self, point: WorldPoint) -> WorldPoint {
        WorldPoint::new(
            point.x.clamp(self.x, self.right()),
            point.y.clamp(self.y, self.bottom()),
        )
    }

    pub fn overlaps(self, other: Rect) -> bool {
        let overlap_x = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let overlap_y = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        overlap_x > OVERLAP_EPSILON && overlap_y > OVERLAP_EPSILON
    }

    pub fn adjacent(self, other: Rect) -> bool {
        let vertical_span = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        let horizontal_span = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let vertical_touch = (self.right() - other.x).abs() <= ADJACENCY_EPSILON
            || (other.right() - self.x).abs() <= ADJACENCY_EPSILON;
        let horizontal_touch = (self.bottom() - other.y).abs() <= ADJACENCY_EPSILON
            || (other.bottom() - self.y).abs() <= ADJACENCY_EPSILON;
        (vertical_touch && vertical_span >= MIN_OVERLAP_SPAN)
            || (horizontal_touch && horizontal_span >= MIN_OVERLAP_SPAN)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Host,
    Device(DeviceId),
}

#[derive(Clone, Debug)]
pub struct ScreenSpec {
    pub id: DeviceId,
    pub label: String,
    pub rect: Rect,
    pub online: bool,
}

#[derive(Clone, Debug)]
pub struct WorldLayout {
    pub host: Rect,
    pub screens: Vec<ScreenSpec>,
}

impl WorldLayout {
    pub fn host_rect(screen: ScreenInfo) -> Rect {
        screen_rect(screen, 0.0, 0.0)
    }

    pub fn new(host: ScreenInfo, screens: Vec<ScreenSpec>) -> Self {
        Self {
            host: Self::host_rect(host),
            screens,
        }
    }

    pub fn screen(&self, id: ScreenId) -> Option<Rect> {
        match id {
            ScreenId::Host => Some(self.host),
            ScreenId::Device(device) => self
                .screens
                .iter()
                .find(|screen| screen.id == device)
                .map(|screen| screen.rect),
        }
    }

    pub fn screen_spec(&self, id: ScreenId) -> Option<&ScreenSpec> {
        match id {
            ScreenId::Host => None,
            ScreenId::Device(device) => self.screens.iter().find(|screen| screen.id == device),
        }
    }

    pub fn is_online(&self, id: ScreenId) -> bool {
        match id {
            ScreenId::Host => true,
            ScreenId::Device(device) => self
                .screens
                .iter()
                .any(|screen| screen.id == device && screen.online),
        }
    }

    /// Return the screen containing `point`, excluding `exclude`.
    pub fn target_at(&self, point: WorldPoint, exclude: Option<ScreenId>) -> Option<ScreenId> {
        if exclude != Some(ScreenId::Host) && self.host.contains(point) {
            return Some(ScreenId::Host);
        }
        let mut best: Option<(ScreenId, f32)> = None;
        for screen in &self.screens {
            if !screen.online || screen.id == [0u8; 16] {
                continue;
            }
            if exclude == Some(ScreenId::Device(screen.id)) || !screen.rect.contains(point) {
                continue;
            }
            let center = screen.rect.center();
            let distance = (point.x - center.x).powi(2) + (point.y - center.y).powi(2);
            if best.map_or(true, |(_, best_distance)| distance < best_distance) {
                best = Some((ScreenId::Device(screen.id), distance));
            }
        }
        best.map(|(id, _)| id)
    }

    pub fn online_device_at(&self, point: WorldPoint) -> Option<DeviceId> {
        self.screens
            .iter()
            .filter(|screen| screen.online)
            .filter(|screen| screen.rect.contains(point))
            .min_by(|a, b| {
                let ca = a.rect.center();
                let cb = b.rect.center();
                let da = (point.x - ca.x).powi(2) + (point.y - ca.y).powi(2);
                let db = (point.x - cb.x).powi(2) + (point.y - cb.y).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|screen| screen.id)
    }

    pub fn world_to_host_norm(&self, point: WorldPoint) -> NormPoint {
        NormPoint::new(
            (point.x - self.host.x) / self.host.w.max(f32::EPSILON),
            (point.y - self.host.y) / self.host.h.max(f32::EPSILON),
        )
        .clamped()
    }

    pub fn world_to_local(&self, id: ScreenId, point: WorldPoint) -> Option<NormPoint> {
        let rect = self.screen(id)?;
        Some(
            NormPoint::new(
                (point.x - rect.x) / rect.w.max(f32::EPSILON),
                (point.y - rect.y) / rect.h.max(f32::EPSILON),
            )
            .inset(),
        )
    }

    pub fn local_to_world(&self, id: ScreenId, local: NormPoint) -> Option<WorldPoint> {
        let rect = self.screen(id)?;
        Some(WorldPoint::new(
            rect.x + local.x.clamp(0.0, 1.0) * rect.w,
            rect.y + local.y.clamp(0.0, 1.0) * rect.h,
        ))
    }

    pub fn clamp_to_screen(&self, id: ScreenId, point: WorldPoint) -> Option<WorldPoint> {
        self.screen(id).map(|rect| rect.clamp(point))
    }

    pub fn has_overlaps(&self) -> bool {
        if self
            .screens
            .iter()
            .any(|screen| screen.rect.overlaps(self.host))
        {
            return true;
        }
        for (index, screen) in self.screens.iter().enumerate() {
            if self.screens[index + 1..]
                .iter()
                .any(|other| screen.rect.overlaps(other.rect))
            {
                return true;
            }
        }
        false
    }

    pub fn set_online(&mut self, device: DeviceId, online: bool) -> bool {
        if let Some(screen) = self.screens.iter_mut().find(|screen| screen.id == device) {
            if screen.online != online {
                screen.online = online;
                return true;
            }
        }
        false
    }

    pub fn set_rect(&mut self, device: DeviceId, rect: Rect) -> bool {
        if let Some(screen) = self.screens.iter_mut().find(|screen| screen.id == device) {
            if screen.rect != rect {
                screen.rect = rect;
                return true;
            }
        }
        false
    }

    pub fn adjacent_devices(&self) -> Vec<(DeviceId, DeviceId)> {
        let mut out = Vec::new();
        for (index, screen) in self.screens.iter().enumerate() {
            for other in &self.screens[index + 1..] {
                if screen.rect.adjacent(other.rect) {
                    out.push((screen.id, other.id));
                }
            }
        }
        out
    }
}

pub fn screen_rect(screen: ScreenInfo, x: f32, y: f32) -> Rect {
    let aspect = if screen.width == 0 || screen.height == 0 {
        9.0 / 16.0
    } else {
        screen.height as f32 / screen.width as f32
    };
    Rect::new(x, y, 1.0, aspect.clamp(0.25, 4.0))
}

pub fn host_px_to_world(px: f64, py: f64, origin_x: i32, origin_y: i32, host_w: i32) -> WorldPoint {
    let scale = host_w.max(1) as f64;
    WorldPoint::new(
        ((px - origin_x as f64) / scale) as f32,
        ((py - origin_y as f64) / scale) as f32,
    )
}

pub fn world_to_host_px(
    point: WorldPoint,
    origin_x: i32,
    origin_y: i32,
    host_w: i32,
    host_h: i32,
) -> (f64, f64) {
    let x = (point.x as f64 * host_w as f64 + origin_x as f64)
        .clamp(origin_x as f64, (origin_x + host_w.max(2) - 1) as f64);
    let y = (point.y as f64 * host_w as f64 + origin_y as f64)
        .clamp(origin_y as f64, (origin_y + host_h.max(2) - 1) as f64);
    (x, y)
}

pub fn advance_world(point: WorldPoint, dx: f64, dy: f64, host_w: i32) -> WorldPoint {
    let scale = host_w.max(1) as f64;
    WorldPoint::new(point.x + (dx / scale) as f32, point.y + (dy / scale) as f32)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepResult {
    Stay {
        local: NormPoint,
        world: WorldPoint,
    },
    Switch {
        target: DeviceId,
        local: NormPoint,
        world: WorldPoint,
        consumed: WorldPoint,
    },
    ReturnHost {
        local: NormPoint,
        world: WorldPoint,
        consumed: WorldPoint,
    },
    Wall {
        local: NormPoint,
        world: WorldPoint,
    },
}

/// Advance while the pointer is on an agent screen. A gap between screens is
/// a wall; entering any other online screen rect switches directly.
pub fn step_active(
    layout: &WorldLayout,
    target: DeviceId,
    world: WorldPoint,
    dx: f64,
    dy: f64,
    host_w: i32,
) -> StepResult {
    let next = advance_world(world, dx, dy, host_w);
    let Some(current_rect) = layout.screen(ScreenId::Device(target)) else {
        return StepResult::ReturnHost {
            local: NormPoint::new(0.5, 0.5),
            world: layout.host.center(),
            consumed: world,
        };
    };

    if current_rect.contains(next) {
        return StepResult::Stay {
            local: layout
                .world_to_local(ScreenId::Device(target), next)
                .unwrap_or_default(),
            world: next,
        };
    }

    let consumed = current_rect.clamp(next);
    match layout.target_at(next, Some(ScreenId::Device(target))) {
        Some(ScreenId::Host) => {
            let local = layout.world_to_host_norm(next).inset();
            let world = layout
                .local_to_world(ScreenId::Host, local)
                .unwrap_or(layout.host.center());
            StepResult::ReturnHost {
                local,
                world,
                consumed,
            }
        }
        Some(ScreenId::Device(device)) => {
            let local = layout
                .world_to_local(ScreenId::Device(device), next)
                .unwrap_or_default()
                .inset();
            let world = layout
                .local_to_world(ScreenId::Device(device), local)
                .unwrap_or(next);
            StepResult::Switch {
                target: device,
                local,
                world,
                consumed,
            }
        }
        None => {
            let clamped = current_rect.clamp(next);
            StepResult::Wall {
                local: layout
                    .world_to_local(ScreenId::Device(target), clamped)
                    .unwrap_or_default(),
                world: clamped,
            }
        }
    }
}

/// Try to enter an online agent screen from the host edge.
pub fn enter_from_host(
    layout: &WorldLayout,
    host_world: WorldPoint,
    dx: f64,
    dy: f64,
    host_w: i32,
) -> Option<(DeviceId, NormPoint, WorldPoint)> {
    let next = advance_world(host_world, dx, dy, host_w);
    let device = layout.online_device_at(next)?;
    let local = layout
        .world_to_local(ScreenId::Device(device), next)
        .unwrap_or_default()
        .inset();
    let world = layout
        .local_to_world(ScreenId::Device(device), local)
        .unwrap_or(next);
    Some((device, local, world))
}

/// Place a new screen so its edge touches the host or another screen.
pub fn default_placement(host: Rect, screens: &[ScreenSpec], new_screen: ScreenInfo) -> (f32, f32) {
    let height = screen_rect(new_screen, 0.0, 0.0).h;
    let candidates = [
        (host.right(), host.y),
        (host.x - 1.0, host.y),
        (host.x, host.bottom()),
        (host.x, host.y - height),
    ];
    for (x, y) in candidates {
        let rect = Rect::new(x, y, 1.0, height);
        if !rect.overlaps(host) && screens.iter().all(|screen| !rect.overlaps(screen.rect)) {
            return (x, y);
        }
    }

    let mut y = host.bottom();
    for _ in 0..64 {
        let rect = Rect::new(host.right(), y, 1.0, height);
        if screens.iter().all(|screen| !rect.overlaps(screen.rect)) {
            return (host.right(), y);
        }
        y += height.max(0.05);
    }
    (host.right(), y)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePlacement {
    pub id: String,
    pub label: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub screen: Option<ScreenInfo>,
}

impl DevicePlacement {
    pub fn device_id(&self) -> Option<DeviceId> {
        device_id_from_hex(&self.id)
    }

    pub fn rect(&self) -> Option<Rect> {
        self.device_id()?;
        Some(screen_rect(
            self.screen.unwrap_or(ScreenInfo {
                width: 1920,
                height: 1080,
            }),
            self.x,
            self.y,
        ))
    }
}

pub fn world_from_placements(
    host: ScreenInfo,
    placements: &[DevicePlacement],
    online: &[DeviceId],
) -> WorldLayout {
    let mut screens = Vec::new();
    for placement in placements {
        let Some(device) = placement.device_id() else {
            continue;
        };
        let rect = placement.rect().unwrap_or_else(|| {
            screen_rect(
                ScreenInfo {
                    width: 1920,
                    height: 1080,
                },
                0.0,
                0.0,
            )
        });
        screens.push(ScreenSpec {
            id: device,
            label: placement.label.clone(),
            rect,
            online: online.contains(&device),
        });
    }
    WorldLayout::new(host, screens)
}

/// Shared snapshot holder used by the controller network loop and capture.
#[derive(Clone)]
pub struct LayoutStore {
    inner: Arc<LayoutInner>,
}

struct LayoutInner {
    generation: AtomicU64,
    layout: RwLock<Arc<WorldLayout>>,
}

impl LayoutStore {
    pub fn new(layout: WorldLayout) -> Self {
        Self {
            inner: Arc::new(LayoutInner {
                generation: AtomicU64::new(1),
                layout: RwLock::new(Arc::new(layout)),
            }),
        }
    }

    pub fn snapshot(&self) -> (u64, Arc<WorldLayout>) {
        let generation = self.inner.generation.load(Ordering::Acquire);
        let layout = self
            .inner
            .layout
            .read()
            .map(|layout| Arc::clone(&layout))
            .unwrap_or_default();
        (generation, layout)
    }

    pub fn replace(&self, layout: WorldLayout) {
        if let Ok(mut slot) = self.inner.layout.write() {
            *slot = Arc::new(layout);
            self.inner.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn update<F>(&self, mutate: F)
    where
        F: FnOnce(&mut WorldLayout) -> bool,
    {
        if let Ok(mut slot) = self.inner.layout.write() {
            let mut next = (**slot).clone();
            let changed = mutate(&mut next);
            if changed {
                *slot = Arc::new(next);
                self.inner.generation.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

impl Default for WorldLayout {
    fn default() -> Self {
        Self::new(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            Vec::new(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: u8, x: f32, y: f32) -> ScreenSpec {
        ScreenSpec {
            id: [id; 16],
            label: format!("agent-{id}"),
            rect: screen_rect(
                ScreenInfo {
                    width: 1920,
                    height: 1080,
                },
                x,
                y,
            ),
            online: true,
        }
    }

    fn host() -> Rect {
        WorldLayout::host_rect(ScreenInfo {
            width: 1920,
            height: 1080,
        })
    }

    #[test]
    fn world_mapping_round_trips() {
        let host = host();
        assert!((host.w - 1.0).abs() < f32::EPSILON);
        assert!((host.h - 9.0 / 16.0).abs() < 1e-6);
        let world = WorldLayout::new(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            vec![screen(1, 1.0, 0.0)],
        );
        let local = NormPoint::new(0.25, 0.75);
        let point = world
            .local_to_world(ScreenId::Device([1; 16]), local)
            .unwrap();
        let back = world
            .world_to_local(ScreenId::Device([1; 16]), point)
            .unwrap();
        assert!((back.x - local.x).abs() < 1e-5);
        assert!((back.y - local.y).abs() < 1e-5);
    }

    #[test]
    fn direct_device_to_device_crossing() {
        let world = WorldLayout::new(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            vec![screen(1, 1.0, 0.0), screen(2, 2.0, 0.0)],
        );
        let p = WorldPoint::new(2.001, 0.2);
        assert_eq!(
            world.target_at(p, Some(ScreenId::Device([1; 16]))),
            Some(ScreenId::Device([2; 16]))
        );
        let p = WorldPoint::new(0.99, 0.2);
        assert_eq!(
            world.target_at(p, Some(ScreenId::Device([1; 16]))),
            Some(ScreenId::Host)
        );
    }

    #[test]
    fn gaps_are_walls_and_overlap_is_flagged() {
        let world = WorldLayout::new(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            vec![screen(1, 1.2, 0.0)],
        );
        let p = WorldPoint::new(1.1, 0.2);
        assert_eq!(world.target_at(p, Some(ScreenId::Host)), None);
        let overlapping = WorldLayout::new(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            vec![screen(1, 0.9, 0.0)],
        );
        assert!(overlapping.has_overlaps());
    }

    #[test]
    fn host_pixel_mapping_uses_host_width_for_both_axes() {
        let p = host_px_to_world(960.0, 540.0, 0, 0, 1920);
        assert!((p.x - 0.5).abs() < 1e-6);
        assert!((p.y - 0.28125).abs() < 1e-6);
        let (x, y) = world_to_host_px(p, 0, 0, 1920, 1080);
        assert!((x - 960.0).abs() < 1.0);
        assert!((y - 540.0).abs() < 1.0);
    }

    #[test]
    fn default_placement_avoids_overlap() {
        let host = host();
        let first = screen(1, 1.0, 0.0);
        let (x, y) = default_placement(
            host,
            &[first.clone()],
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
        );
        let rect = screen_rect(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            x,
            y,
        );
        assert!(!rect.overlaps(host));
        assert!(!rect.overlaps(first.rect));
    }

    #[test]
    fn adjacency_requires_edge_contact_and_span() {
        let a = screen_rect(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            0.0,
            0.0,
        );
        let b = screen_rect(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            1.0,
            0.0,
        );
        assert!(a.adjacent(b));
        let gapped = screen_rect(
            ScreenInfo {
                width: 1920,
                height: 1080,
            },
            1.1,
            0.0,
        );
        assert!(!a.adjacent(gapped));
    }

    #[test]
    fn device_id_hex_round_trip() {
        let id = new_device_id();
        let hex = device_id_to_hex(&id);
        assert_eq!(device_id_from_hex(&hex), Some(id));
        assert_eq!(device_id_from_hex("bad"), None);
    }
}
