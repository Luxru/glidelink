mod metrics;
pub mod protocol;
mod stream;

use crate::config::{AppConfig, ConfigHandle};
use crate::error;
use crate::input::{
    self, adapt_event, adapt_reconcile, CaptureMsg, InputEvent, InputMsg, PointerPosition,
    ReconcileState,
};
use crate::invite::{self, Invite, PORT_RANGE_END, PORT_RANGE_START};
use crate::layout::{
    default_placement, device_id_to_hex, DeviceId, DevicePlacement, LayoutStore, WorldLayout,
};
use crate::status::{PeerStatus, ScreenInfo, Status};
use crossbeam_channel::Receiver;
use metrics::RttEstimator;
use protocol::{
    build_pake_accept, build_pake_hello, decode_u64, derive_session_keys, encode_u64,
    parse_pake_accept, parse_pake_hello, random_sid, transcript_hash, Handshake, Session, Side,
    CONFIRM_PAYLOAD, TYPE_BYE, TYPE_CONFIRM, TYPE_CONFIRM_ACK, TYPE_PING, TYPE_PONG,
};
use rand::RngCore;
use serde::Serialize;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use stream::FramedStream;

const POLL_INTERVAL: Duration = Duration::from_millis(1);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const PING_ACTIVE_INTERVAL: Duration = Duration::from_millis(100);
const PING_IDLE_INTERVAL: Duration = Duration::from_millis(500);
const RECONCILE_INTERVAL: Duration = Duration::from_millis(500);
const SESSION_TIMEOUT: Duration = Duration::from_secs(3);
const INPUT_INTERVAL: Duration = Duration::from_millis(4);
const SLOW_INPUT_INTERVAL: Duration = Duration::from_millis(8);
const MAX_PEERS: usize = 8;
const MAX_PENDING: usize = 8;
const POLL_BUDGET: usize = 64;
const PAKE_ID_CONTROLLER: &[u8] = b"GlideLink v1 controller";
const PAKE_ID_AGENT: &[u8] = b"GlideLink v1 agent";

type NetResult<T> = Result<T, String>;

fn local_screen_or_zero() -> ScreenInfo {
    input::screen_info().unwrap_or(ScreenInfo {
        width: 0,
        height: 0,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
    pub name: String,
    pub ip: String,
}

pub fn local_interfaces() -> Vec<NetworkInterface> {
    let mut interfaces = Vec::new();
    if let Ok(adapters) = if_addrs::get_if_addrs() {
        for adapter in adapters {
            if let if_addrs::IfAddr::V4(v4) = adapter.addr {
                if !v4.ip.is_loopback() && !v4.ip.is_unspecified() {
                    interfaces.push(NetworkInterface {
                        name: adapter.name,
                        ip: v4.ip.to_string(),
                    });
                }
            }
        }
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name).then(a.ip.cmp(&b.ip)));
    interfaces.dedup_by(|a, b| a.name == b.name && a.ip == b.ip);
    interfaces
}

struct PakeLimiter {
    tokens: f64,
    last: Instant,
}

impl PakeLimiter {
    fn new() -> Self {
        Self {
            tokens: 8.0,
            last: Instant::now(),
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * 4.0).min(8.0);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

fn bind_listener(ip: Ipv4Addr, last_code: &str) -> NetResult<(TcpListener, String)> {
    if let Ok(invite) = Invite::decode(last_code) {
        if invite.ip == ip {
            if let Ok(listener) = TcpListener::bind((ip, invite.port)) {
                return Ok((listener, last_code.to_string()));
            }
        }
    }
    let count = u32::from(PORT_RANGE_END - PORT_RANGE_START + 1);
    let offset = rand::rngs::OsRng.next_u32() % count;
    for step in 0..count {
        let port = PORT_RANGE_START + ((offset + step) % count) as u16;
        match TcpListener::bind((ip, port)) {
            Ok(listener) => return Ok((listener, Invite::generate(ip, port).encode())),
            Err(e) if e.kind() == ErrorKind::AddrInUse => continue,
            Err(e) => return Err(error::detail("E_NET_BIND", e)),
        }
    }
    Err(error::code("E_PORT_RANGE"))
}

pub fn preview_listen_code(ip: Ipv4Addr, last_code: &str) -> NetResult<String> {
    let (_listener, code) = bind_listener(ip, last_code)?;
    Ok(code)
}

fn platform_label(id: u8) -> &'static str {
    match id {
        1 => "macOS",
        2 => "Windows",
        _ => "Unknown",
    }
}

fn peer_label(platform: u8, addr: SocketAddr) -> String {
    format!("{} · {}", platform_label(platform), addr.ip())
}

/// Shared by both roles: one cipher sequence, one framed stream, one heartbeat.
struct Link {
    wire: FramedStream,
    session: Session,
    last_received: Instant,
    last_ping: Instant,
    pending_pings: Vec<(u64, Instant)>,
    next_ping: u64,
    rtt: RttEstimator,
    last_log: Instant,
}

impl Link {
    fn new(wire: FramedStream, session: Session) -> Self {
        let now = Instant::now();
        Self {
            wire,
            session,
            last_received: now,
            last_ping: now.checked_sub(PING_IDLE_INTERVAL).unwrap_or(now),
            pending_pings: Vec::new(),
            next_ping: 1,
            rtt: RttEstimator::new(),
            last_log: now,
        }
    }

    fn send(&mut self, kind: u8, payload: &[u8]) -> NetResult<()> {
        self.wire
            .queue(self.session.encrypt_packet(kind, payload))
            .map_err(|e| e.to_string())
    }

    fn send_input(&mut self, input: InputMsg) -> NetResult<()> {
        let (kind, payload) = protocol::encode_command(input).map_err(|e| e.to_string())?;
        self.send(kind, &payload)
    }

    fn receive(&mut self) -> NetResult<Option<(u8, Vec<u8>)>> {
        let Some(packet) = self.wire.read_packet().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let message = self
            .session
            .decrypt_packet(&packet)
            .map_err(|e| e.to_string())?;
        self.last_received = Instant::now();
        Ok(Some(message))
    }

    fn heartbeat(&mut self, kind: u8, payload: &[u8]) -> NetResult<bool> {
        match kind {
            TYPE_PING => {
                let id = decode_u64(payload).map_err(|e| e.to_string())?;
                self.send(TYPE_PONG, &encode_u64(id))?;
            }
            TYPE_PONG => {
                let id = decode_u64(payload).map_err(|e| e.to_string())?;
                if let Some(index) = self
                    .pending_pings
                    .iter()
                    .position(|(pending, _)| *pending == id)
                {
                    let (_, sent) = self.pending_pings.remove(index);
                    self.rtt.observe(sent.elapsed().as_secs_f64() * 1000.0);
                }
            }
            TYPE_BYE => return Err(error::code("NET_PEER_DISCONNECTED")),
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn tick(&mut self, now: Instant, interval: Duration) -> NetResult<()> {
        if now.saturating_duration_since(self.last_received) >= SESSION_TIMEOUT {
            return Err("peer heartbeat timed out".into());
        }
        if now.saturating_duration_since(self.last_ping) >= interval {
            let id = self.next_ping;
            self.next_ping = self.next_ping.wrapping_add(1).max(1);
            self.send(TYPE_PING, &encode_u64(id))?;
            self.pending_pings.push((id, now));
            if self.pending_pings.len() > 32 {
                self.pending_pings.remove(0);
            }
            self.last_ping = now;
        }
        if now.saturating_duration_since(self.last_log) >= Duration::from_secs(5) {
            log::debug!(
                "TCP input queue={} age_ms={:.1} srtt_ms={:?} rttvar_ms={:.1}",
                self.wire.queue_depth(),
                self.wire.queue_age().as_secs_f64() * 1000.0,
                self.rtt.srtt_ms(),
                self.rtt.jitter_ms()
            );
            self.last_log = now;
        }
        self.wire.flush().map_err(|e| e.to_string())
    }
}

struct PeerSession {
    device_id: DeviceId,
    addr: SocketAddr,
    screen: ScreenInfo,
    platform: u8,
    link: Link,
    pending_pointer: Option<PointerPosition>,
    reconcile: ReconcileState,
    last_move: Instant,
    last_reconcile: Instant,
}

fn pointer_interval(rtt: &RttEstimator) -> Duration {
    if rtt.jitter_ms() > 4.0 || rtt.srtt_ms().unwrap_or(0.0) > 40.0 {
        SLOW_INPUT_INTERVAL
    } else {
        INPUT_INTERVAL
    }
}

impl PeerSession {
    fn flush_pointer(&mut self) -> NetResult<()> {
        if let Some(input) = self.pending_pointer.take() {
            self.link.send_input(InputMsg::Pointer(input))?;
            self.last_move = Instant::now();
        }
        Ok(())
    }

    fn capture(&mut self, input: InputMsg, adapt: bool) -> NetResult<()> {
        if let InputMsg::Pointer(position) = input {
            self.pending_pointer = Some(position);
            return Ok(());
        }
        // One common barrier for every semantic command, including activation.
        self.flush_pointer()?;
        let wire = match input {
            InputMsg::Event(mut event) => {
                apply_event_to_reconcile(&mut self.reconcile, event);
                if adapt {
                    adapt_event(&mut event);
                }
                InputMsg::Event(event)
            }
            InputMsg::Reconcile(mut state) => {
                self.reconcile = state;
                self.last_reconcile = Instant::now();
                if adapt {
                    adapt_reconcile(&mut state);
                }
                InputMsg::Reconcile(state)
            }
            other => other,
        };
        self.link.send_input(wire)
    }

    fn poll(&mut self, now: Instant, active: bool, adapt: bool) -> NetResult<()> {
        for _ in 0..POLL_BUDGET {
            let Some((kind, payload)) = self.link.receive()? else {
                break;
            };
            if !self.link.heartbeat(kind, &payload)? {
                return Err("unexpected agent message".into());
            }
        }
        if now.saturating_duration_since(self.last_move) >= pointer_interval(&self.link.rtt) {
            self.flush_pointer()?;
        }
        if now.saturating_duration_since(self.last_reconcile) >= RECONCILE_INTERVAL {
            let mut state = self.reconcile;
            state.active = active;
            if !active {
                state.buttons = 0;
                state.keys = input::keymap::KeyState::EMPTY;
            }
            self.capture(InputMsg::Reconcile(state), adapt)?;
        }
        self.link.tick(
            now,
            if active {
                PING_ACTIVE_INTERVAL
            } else {
                PING_IDLE_INTERVAL
            },
        )
    }
}

fn apply_event_to_reconcile(state: &mut ReconcileState, event: InputEvent) {
    match event {
        InputEvent::Button { button, down } => {
            if down {
                state.buttons |= button;
            } else {
                state.buttons &= !button;
            }
        }
        InputEvent::Key { hid, down } => {
            if down {
                state.keys.insert(hid);
            } else {
                state.keys.remove(hid);
            }
        }
        InputEvent::CapsLock { locked } => state.caps_lock = locked,
        InputEvent::Wheel { .. } => {}
    }
}

struct PendingConnection {
    wire: FramedStream,
    addr: SocketAddr,
    created: Instant,
    auth: Option<(Handshake, Session)>,
}

impl PendingConnection {
    fn poll(&mut self, code: &str, local_device: DeviceId, screen: ScreenInfo) -> NetResult<bool> {
        if self.created.elapsed() >= HANDSHAKE_TIMEOUT {
            return Err("handshake timed out".into());
        }
        self.wire.flush().map_err(|e| e.to_string())?;
        let Some(packet) = self.wire.read_packet().map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        if let Some((_, session)) = self.auth.as_mut() {
            let (kind, payload) = session.decrypt_packet(&packet).map_err(|e| e.to_string())?;
            if kind != TYPE_CONFIRM || payload != CONFIRM_PAYLOAD {
                return Err("invalid confirmation".into());
            }
            self.wire
                .queue(session.encrypt_packet(TYPE_CONFIRM_ACK, CONFIRM_PAYLOAD))
                .map_err(|e| e.to_string())?;
            return Ok(true);
        }
        let (hello, pake_msg) = parse_pake_hello(&packet).map_err(|e| e.to_string())?;
        if hello.device_id == [0; 16]
            || hello.device_id == local_device
            || hello.screen.width == 0
            || hello.screen.height == 0
        {
            return Err("invalid peer identity or desktop".into());
        }
        let (pake, accept_msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_bytes()),
            &Identity::new(PAKE_ID_CONTROLLER),
            &Identity::new(PAKE_ID_AGENT),
        );
        let shared = pake.finish(&pake_msg).map_err(|_| error::code("E_PAKE"))?;
        let accept = Handshake {
            sid: hello.sid,
            screen,
            timestamp_ms: protocol::now_ms(),
            platform: input::platform_id(),
            device_id: local_device,
        };
        let accept_packet = build_pake_accept(&accept, &accept_msg);
        let transcript = transcript_hash(&packet, &accept_packet);
        let (c2a, a2c) = derive_session_keys(&shared, &hello.sid, &transcript);
        self.wire.queue(accept_packet).map_err(|e| e.to_string())?;
        self.auth = Some((
            hello.clone(),
            Session::from_keys(&c2a, &a2c, hello.sid, Side::Controller),
        ));
        self.wire.flush().map_err(|e| e.to_string())?;
        Ok(false)
    }
}

struct Hub {
    listener: TcpListener,
    pairing_code: String,
    local_screen: ScreenInfo,
    local_device: DeviceId,
    adapt_modifiers: bool,
    config: ConfigHandle,
    layout: LayoutStore,
    status: Status,
    peers: HashMap<DeviceId, PeerSession>,
    pending: Vec<PendingConnection>,
    active_target: Option<DeviceId>,
    limiters: HashMap<IpAddr, PakeLimiter>,
    last_status_refresh: Instant,
}

impl Hub {
    fn handle_capture(&mut self, message: CaptureMsg) {
        let device = message.device;
        if !self
            .peers
            .get(&device)
            .is_some_and(|peer| peer.link.session.id() == message.session)
        {
            return;
        }
        if let InputMsg::Reconcile(state) = message.input {
            if state.active && self.active_target != Some(device) {
                if let Some(old) = self.active_target {
                    if let Some(peer) = self.peers.get_mut(&old) {
                        if let Err(e) =
                            peer.capture(InputMsg::Reconcile(ReconcileState::INACTIVE), false)
                        {
                            log::warn!("release send failed: {e}");
                            self.remove_peer(old);
                        }
                    }
                }
                self.active_target = Some(device);
            } else if !state.active && self.active_target == Some(device) {
                self.active_target = None;
            }
        }
        if let Some(peer) = self.peers.get_mut(&device) {
            let adapt = self.adapt_modifiers && peer.platform != input::platform_id();
            if let Err(e) = peer.capture(message.input, adapt) {
                log::warn!("input connection closed: {e}");
                self.remove_peer(device);
            }
        }
    }

    fn accept_connections(&mut self) {
        // Limit work per pass so connection attempts cannot starve input.
        for _ in 0..MAX_PENDING {
            match self.listener.accept() {
                Ok((socket, addr)) => {
                    if self.pending.len() >= MAX_PENDING
                        || !self
                            .limiters
                            .entry(addr.ip())
                            .or_insert_with(PakeLimiter::new)
                            .allow()
                    {
                        continue;
                    }
                    match FramedStream::new(socket, self.status.clone()) {
                        Ok(wire) => self.pending.push(PendingConnection {
                            wire,
                            addr,
                            created: Instant::now(),
                            auth: None,
                        }),
                        Err(e) => log::debug!("TCP setup failed: {e}"),
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    log::warn!("TCP accept failed: {e}");
                    break;
                }
            }
        }
    }

    fn promote(&mut self, mut pending: PendingConnection) -> NetResult<()> {
        let (hello, session) = pending.auth.take().ok_or("missing authentication")?;
        let device = hello.device_id;
        if self.peers.len() >= MAX_PEERS && !self.peers.contains_key(&device) {
            return Err("peer limit reached".into());
        }
        // A new stream is a fresh input lifecycle. Never restore held keys or
        // pending positions from a previous connection with the same device id.
        self.remove_peer(device);
        let now = Instant::now();
        let mut peer = PeerSession {
            device_id: device,
            addr: pending.addr,
            screen: hello.screen,
            platform: hello.platform,
            link: Link::new(pending.wire, session),
            pending_pointer: None,
            reconcile: ReconcileState::INACTIVE,
            last_move: now,
            last_reconcile: now,
        };
        peer.link
            .send_input(InputMsg::Reconcile(ReconcileState::INACTIVE))?;
        peer.link.wire.flush().map_err(|e| e.to_string())?;
        self.peers.insert(device, peer);
        self.ensure_placement(device, hello.platform, pending.addr, hello.screen);
        self.refresh_layout();
        self.refresh_status();
        log::info!(
            "agent {} connected via TCP ({}x{}, {})",
            peer_label(hello.platform, pending.addr),
            hello.screen.width,
            hello.screen.height,
            device_id_to_hex(&device)
        );
        Ok(())
    }
    fn ensure_placement(
        &mut self,
        device: DeviceId,
        platform: u8,
        addr: SocketAddr,
        screen: ScreenInfo,
    ) {
        let label = peer_label(platform, addr);
        let (_, current) = self.layout.snapshot();
        let host = WorldLayout::host_rect(self.local_screen);
        let saved = self.config.snapshot().unwrap_or_default();
        let mut found = false;
        let mut placements = saved.layout.clone();
        for placement in placements.iter_mut() {
            if placement.device_id() == Some(device) {
                placement.label = label.clone();
                placement.screen = Some(screen);
                found = true;
                break;
            }
        }
        if !found {
            let (x, y) = default_placement(host, &current.screens, screen);
            placements.push(DevicePlacement {
                id: device_id_to_hex(&device),
                label,
                x,
                y,
                screen: Some(screen),
            });
        }
        if let Err(e) = self.config.update(|config| {
            config.layout = placements;
            Ok(())
        }) {
            log::warn!("could not persist device placement: {e}");
        }
    }

    fn refresh_layout(&mut self) {
        let config = self.config.snapshot().unwrap_or_default();
        let connections: Vec<_> = self
            .peers
            .iter()
            .map(|(id, peer)| (*id, peer.link.session.id()))
            .collect();
        let world =
            crate::layout::world_from_placements(self.local_screen, &config.layout, &connections);
        self.layout.replace(world);
    }

    fn remove_peer(&mut self, device: DeviceId) {
        if let Some(peer) = self.peers.remove(&device) {
            if self.active_target == Some(device) {
                self.active_target = None;
            }
            self.refresh_layout();
            self.refresh_status();
            log::info!(
                "agent {} disconnected",
                peer_label(peer.platform, peer.addr)
            );
        }
    }

    fn refresh_status(&mut self) {
        self.last_status_refresh = Instant::now();
        let mut devices: Vec<_> = self
            .peers
            .values()
            .map(|peer| PeerStatus {
                device_id: device_id_to_hex(&peer.device_id),
                label: peer_label(peer.platform, peer.addr),
                addr: peer.addr.to_string(),
                platform: Some(peer.platform),
                screen: Some(peer.screen),
                online: true,
                active: self.active_target == Some(peer.device_id),
                latency_ms: peer.link.rtt.srtt_ms(),
                jitter_ms: peer.link.rtt.srtt_ms().map(|_| peer.link.rtt.jitter_ms()),
                packet_loss_percent: None,
                last_error_code: None,
            })
            .collect();
        devices.sort_by(|a, b| a.label.cmp(&b.label));
        let active = self.active_target.and_then(|id| self.peers.get(&id));
        self.status.update(|s| {
            s.running = true;
            s.connected = !devices.is_empty();
            s.remote_active = active.is_some();
            s.devices = devices;
            s.target_device = self.active_target.map(|id| device_id_to_hex(&id));
            s.peer = active.map(|p| peer_label(p.platform, p.addr));
            s.peer_platform = active.map(|p| p.platform);
            s.remote_screen = active.map(|p| p.screen);
            s.latency_ms = active.and_then(|p| p.link.rtt.srtt_ms());
            s.jitter_ms = active.and_then(|p| p.link.rtt.srtt_ms().map(|_| p.link.rtt.jitter_ms()));
            s.packet_loss_percent = None;
            s.input_interval_ms =
                active.map(|p| pointer_interval(&p.link.rtt).as_secs_f64() * 1000.0);
        });
    }

    fn poll(&mut self) {
        self.accept_connections();
        for mut pending in std::mem::take(&mut self.pending) {
            match pending.poll(&self.pairing_code, self.local_device, self.local_screen) {
                Ok(true) => {
                    if let Err(e) = self.promote(pending) {
                        log::debug!("TCP promotion failed: {e}");
                    }
                }
                Ok(false) => self.pending.push(pending),
                Err(e) => log::debug!("handshake from {} rejected: {e}", pending.addr),
            }
        }
        let now = Instant::now();
        let mut remove = Vec::new();
        for (device, peer) in &mut self.peers {
            let adapt = self.adapt_modifiers && peer.platform != input::platform_id();
            if let Err(e) = peer.poll(now, self.active_target == Some(*device), adapt) {
                log::debug!("input connection {} closed: {e}", peer.addr);
                remove.push(*device);
            }
        }
        for device in remove {
            self.remove_peer(device);
        }
        self.limiters
            .retain(|_, limiter| limiter.last.elapsed() < Duration::from_secs(30));
        if now.saturating_duration_since(self.last_status_refresh) >= Duration::from_millis(100) {
            self.refresh_status();
        }
    }

    fn shutdown(&mut self) {
        for peer in self.peers.values_mut() {
            let _ = peer.capture(InputMsg::Reconcile(ReconcileState::INACTIVE), false);
            let _ = peer.link.send(TYPE_BYE, &[]);
            let _ = peer.link.wire.flush();
        }
    }
}

pub fn start_controller(
    config: &AppConfig,
    config_handle: ConfigHandle,
    layout: LayoutStore,
    capture_rx: Receiver<CaptureMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> NetResult<JoinHandle<()>> {
    let ip = config.local_ipv4()?;
    let (listener, pairing_code) = bind_listener(ip, &config.last_invite)?;
    listener
        .set_nonblocking(true)
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;
    if let Err(e) = config_handle.update(|c| {
        c.last_invite = pairing_code.clone();
        Ok(())
    }) {
        log::warn!("could not persist listening code: {e}");
    }
    let local_screen = local_screen_or_zero();
    let local_device = config.device_id_bytes()?;
    status.update(|s| {
        s.running = true;
        s.connected = false;
        s.remote_active = false;
        s.local_screen = Some(local_screen);
        s.peer = None;
        s.peer_platform = None;
        s.remote_screen = None;
        s.latency_ms = None;
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.last_error_code = None;
        s.last_error_detail = None;
        s.pairing_code = Some(pairing_code.clone());
        s.devices.clear();
        s.target_device = None;
        s.started_at_ms = Some(protocol::now_ms());
    });
    let hub = Hub {
        listener,
        pairing_code,
        local_screen,
        local_device,
        adapt_modifiers: config.adapt_modifiers,
        config: config_handle,
        layout,
        status,
        peers: HashMap::new(),
        pending: Vec::new(),
        active_target: None,
        limiters: HashMap::new(),
        last_status_refresh: Instant::now(),
    };
    log::info!("controller listening on {ip} via TCP");
    thread::Builder::new()
        .name("glidelink-controller-hub".into())
        .spawn(move || run_hub(hub, capture_rx, stop))
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

fn run_hub(mut hub: Hub, capture_rx: Receiver<CaptureMsg>, stop: Arc<AtomicBool>) {
    crate::sched::boost_current_thread();
    let _guard = crate::sched::SessionGuard::begin();
    while !stop.load(Ordering::Acquire) {
        // Bounded work prevents a busy mouse or peer from starving the others.
        for _ in 0..POLL_BUDGET {
            match capture_rx.try_recv() {
                Ok(message) => hub.handle_capture(message),
                Err(_) => break,
            }
        }
        hub.poll();
        thread::sleep(POLL_INTERVAL);
    }
    hub.shutdown();
    hub.layout.update(|layout| {
        for s in &mut layout.screens {
            s.online = false;
        }
        true
    });
    hub.status.update(|s| {
        s.connected = false;
        s.remote_active = false;
        s.peer = None;
        s.devices.clear();
        s.target_device = None;
    });
}

/// Wait only during PAKE. Normal traffic uses the same nonblocking framing.
fn wait_packet(wire: &mut FramedStream, stop: &AtomicBool) -> NetResult<Vec<u8>> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while !stop.load(Ordering::Acquire) {
        wire.flush().map_err(|e| e.to_string())?;
        if let Some(packet) = wire.read_packet().map_err(|e| e.to_string())? {
            return Ok(packet);
        }
        if Instant::now() >= deadline {
            return Err(error::code("NET_TIMEOUT"));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err("stopped".into())
}

fn connect_agent(
    controller: SocketAddr,
    code: &str,
    local_screen: ScreenInfo,
    local_device: DeviceId,
    stop: &AtomicBool,
    status: &Status,
) -> NetResult<(Link, Handshake)> {
    let socket = TcpStream::connect_timeout(&controller, CONNECT_TIMEOUT)
        .map_err(|e| error::detail("NET_NO_RESPONSE", e))?;
    let mut wire =
        FramedStream::new(socket, status.clone()).map_err(|e| error::detail("E_NET_SOCKET", e))?;
    let sid = random_sid();
    let (pake, hello_msg) = Spake2::<Ed25519Group>::start_b(
        &Password::new(code.as_bytes()),
        &Identity::new(PAKE_ID_CONTROLLER),
        &Identity::new(PAKE_ID_AGENT),
    );
    let hello = Handshake {
        sid,
        screen: local_screen,
        timestamp_ms: protocol::now_ms(),
        platform: input::platform_id(),
        device_id: local_device,
    };
    let hello_packet = build_pake_hello(&hello, &hello_msg);
    wire.queue(hello_packet.clone())
        .map_err(|e| e.to_string())?;
    let accept_packet = wait_packet(&mut wire, stop)?;
    let (accept, accept_msg) =
        parse_pake_accept(&accept_packet, &sid).map_err(|e| e.to_string())?;
    let shared = pake
        .finish(&accept_msg)
        .map_err(|_| error::code("E_PAKE"))?;
    let transcript = transcript_hash(&hello_packet, &accept_packet);
    let (c2a, a2c) = derive_session_keys(&shared, &sid, &transcript);
    let mut session = Session::from_keys(&c2a, &a2c, sid, Side::Agent);
    wire.queue(session.encrypt_packet(TYPE_CONFIRM, CONFIRM_PAYLOAD))
        .map_err(|e| e.to_string())?;
    let ack = wait_packet(&mut wire, stop)?;
    let (kind, payload) = session.decrypt_packet(&ack).map_err(|e| e.to_string())?;
    if kind != TYPE_CONFIRM_ACK || payload != CONFIRM_PAYLOAD {
        return Err(error::code("E_PAKE"));
    }
    Ok((Link::new(wire, session), accept))
}

pub fn start_agent(
    config: &AppConfig,
    pairing_code: &str,
    inject_tx: crossbeam_channel::Sender<InputMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> NetResult<JoinHandle<()>> {
    let invite = Invite::decode(pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    let controller = SocketAddr::new(IpAddr::V4(invite.ip), invite.port);
    let code = invite::normalize_code(pairing_code);
    let local_screen = local_screen_or_zero();
    let local_device = config.device_id_bytes()?;
    status.update(|s| {
        s.running = true;
        s.connected = false;
        s.remote_active = false;
        s.local_screen = Some(local_screen);
        s.remote_screen = None;
        s.peer = Some(controller.to_string());
        s.peer_platform = None;
        s.latency_ms = None;
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.last_error_code = None;
        s.last_error_detail = None;
        s.pairing_code = Some(code.clone());
        s.devices.clear();
        s.target_device = None;
        s.started_at_ms = Some(protocol::now_ms());
    });
    thread::Builder::new()
        .name("glidelink-agent-connect".into())
        .spawn(move || {
            crate::sched::boost_current_thread();
            let _guard = crate::sched::SessionGuard::begin();
            while !stop.load(Ordering::Acquire) {
                let result = connect_agent(
                    controller,
                    &code,
                    local_screen,
                    local_device,
                    &stop,
                    &status,
                )
                .and_then(|(link, accept)| {
                    run_agent(link, accept, controller, &inject_tx, &stop, &status)
                });
                // Reset before backoff, including authentication/transport failures.
                let _ = inject_tx.send(InputMsg::Reset);
                status.update(|s| {
                    s.connected = false;
                    s.remote_active = false;
                    s.latency_ms = None;
                    s.jitter_ms = None;
                    s.devices.clear();
                    s.target_device = None;
                });
                if let Err(e) = result {
                    log::debug!("agent connection ended: {e}");
                    if !stop.load(Ordering::Acquire) {
                        status.set_error("NET_PEER_DISCONNECTED", Some(e));
                    }
                }
                let retry_at = Instant::now() + Duration::from_millis(250);
                while !stop.load(Ordering::Acquire) && Instant::now() < retry_at {
                    thread::sleep(POLL_INTERVAL);
                }
            }
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

fn run_agent(
    mut link: Link,
    accept: Handshake,
    controller: SocketAddr,
    inject_tx: &crossbeam_channel::Sender<InputMsg>,
    stop: &AtomicBool,
    status: &Status,
) -> NetResult<()> {
    let _ = inject_tx.send(InputMsg::Reset);
    status.update(|s| {
        s.connected = true;
        s.remote_active = false;
        s.peer_platform = Some(accept.platform);
        s.remote_screen = Some(accept.screen);
        s.last_error_code = None;
        s.last_error_detail = None;
        s.devices = vec![PeerStatus {
            device_id: device_id_to_hex(&accept.device_id),
            label: peer_label(accept.platform, controller),
            addr: controller.to_string(),
            platform: Some(accept.platform),
            screen: Some(accept.screen),
            online: true,
            ..PeerStatus::default()
        }];
    });
    let mut last_status = Instant::now();
    while !stop.load(Ordering::Acquire) {
        for _ in 0..POLL_BUDGET {
            let Some((kind, payload)) = link.receive()? else {
                break;
            };
            if link.heartbeat(kind, &payload)? {
                continue;
            }
            let input = protocol::decode_command(kind, &payload).map_err(|e| e.to_string())?;
            if let InputMsg::Reconcile(state) = input {
                status.update(|s| {
                    s.remote_active = state.active;
                    if let Some(peer) = s.devices.first_mut() {
                        peer.active = state.active;
                    }
                });
            }
            inject_tx
                .send(input)
                .map_err(|_| "input injector stopped".to_string())?;
        }
        let now = Instant::now();
        link.tick(now, PING_IDLE_INTERVAL)?;
        if now.saturating_duration_since(last_status) >= Duration::from_millis(100) {
            status.update(|s| {
                s.latency_ms = link.rtt.srtt_ms();
                s.jitter_ms = link.rtt.srtt_ms().map(|_| link.rtt.jitter_ms());
                s.packet_loss_percent = None;
                if let Some(peer) = s.devices.first_mut() {
                    peer.latency_ms = s.latency_ms;
                    peer.jitter_ms = s.jitter_ms;
                    peer.packet_loss_percent = None;
                }
            });
            last_status = now;
        }
        thread::sleep(POLL_INTERVAL);
    }
    let _ = link.send(TYPE_BYE, &[]);
    let _ = link.wire.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Role;
    use crate::input::{keymap::KeyState, BUTTON_LEFT};
    use crossbeam_channel::{unbounded, Sender};
    use std::path::PathBuf;

    fn device(byte: u8) -> DeviceId {
        [byte; 16]
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    struct Worker {
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
        status: Status,
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(join) = self.join.take() {
                join.join().unwrap();
            }
        }
    }

    struct Controller {
        worker: Worker,
        tx: Sender<CaptureMsg>,
        code: String,
        path: PathBuf,
        layout: LayoutStore,
    }

    impl Controller {
        fn new() -> Self {
            let config = AppConfig {
                role: Role::Controller,
                local_ip: "127.0.0.1".into(),
                device_id: device_id_to_hex(&device(0x10)),
                ..AppConfig::default()
            };
            let path =
                std::env::temp_dir().join(format!("glidelink-tcp-{}.json", rand::random::<u64>()));
            let handle = ConfigHandle::new(path.clone(), config.clone());
            let layout = LayoutStore::new(WorldLayout::new(
                ScreenInfo {
                    width: 1920,
                    height: 1080,
                },
                vec![],
            ));
            let (tx, rx) = unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            let status = Status::new(None);
            let join = start_controller(
                &config,
                handle,
                layout.clone(),
                rx,
                stop.clone(),
                status.clone(),
            )
            .unwrap();
            let code = status.snapshot().pairing_code.unwrap();
            Self {
                worker: Worker {
                    stop,
                    join: Some(join),
                    status,
                },
                tx,
                code,
                path,
                layout,
            }
        }

        fn agent(&self, byte: u8) -> (Worker, Receiver<InputMsg>) {
            let config = AppConfig {
                role: Role::Agent,
                device_id: device_id_to_hex(&device(byte)),
                ..AppConfig::default()
            };
            let stop = Arc::new(AtomicBool::new(false));
            let status = Status::new(None);
            let (tx, rx) = unbounded();
            let join = start_agent(&config, &self.code, tx, stop.clone(), status.clone()).unwrap();
            assert!(
                wait_until(|| status.snapshot().connected
                    && self
                        .layout
                        .snapshot()
                        .1
                        .screen_spec(crate::layout::ScreenId::Device(device(byte)))
                        .is_some_and(|s| s.online)),
                "agent did not connect: {:?}",
                status.snapshot()
            );
            (
                Worker {
                    stop,
                    join: Some(join),
                    status,
                },
                rx,
            )
        }

        fn send(&self, byte: u8, input: InputMsg) {
            self.tx
                .send(CaptureMsg {
                    device: device(byte),
                    session: self
                        .layout
                        .snapshot()
                        .1
                        .screen_spec(crate::layout::ScreenId::Device(device(byte)))
                        .map_or(0, |screen| screen.session),
                    input,
                })
                .unwrap();
        }
    }

    impl Drop for Controller {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn active() -> InputMsg {
        InputMsg::Reconcile(ReconcileState {
            active: true,
            caps_lock: false,
            buttons: 0,
            keys: KeyState::EMPTY,
        })
    }

    fn pointer(x: f32, y: f32) -> InputMsg {
        InputMsg::Pointer(PointerPosition { x, y })
    }
    fn key(down: bool) -> InputMsg {
        InputMsg::Event(InputEvent::Key { hid: 0x04, down })
    }

    fn recv_matching(
        rx: &Receiver<InputMsg>,
        mut predicate: impl FnMut(InputMsg) -> bool,
    ) -> InputMsg {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let message = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("missing input");
            if predicate(message) {
                return message;
            }
        }
    }

    fn recv_event(rx: &Receiver<InputMsg>) -> InputEvent {
        match recv_matching(rx, |m| matches!(m, InputMsg::Event(_))) {
            InputMsg::Event(e) => e,
            _ => unreachable!(),
        }
    }

    #[test]
    fn two_agents_route_positions_and_semantics_in_order() {
        let controller = Controller::new();
        let (agent1, rx1) = controller.agent(0x21);
        let (agent2, rx2) = controller.agent(0x22);
        assert!(wait_until(|| controller
            .worker
            .status
            .snapshot()
            .devices
            .len()
            == 2));
        assert!(wait_until(|| agent1.status.snapshot().latency_ms.is_some()));
        assert!(agent1.status.snapshot().packet_loss_percent.is_none());

        controller.send(0x21, pointer(0.25, 0.75));
        controller.send(0x21, active());
        let InputMsg::Pointer(pos) = recv_matching(&rx1, |m| matches!(m, InputMsg::Pointer(_)))
        else {
            unreachable!()
        };
        assert!((pos.x - 0.25).abs() < 0.001 && (pos.y - 0.75).abs() < 0.001);
        assert!(!rx2
            .try_iter()
            .any(|m| matches!(m, InputMsg::Pointer(_) | InputMsg::Event(_))));

        let down = InputEvent::Button {
            button: BUTTON_LEFT,
            down: true,
        };
        let up = InputEvent::Button {
            button: BUTTON_LEFT,
            down: false,
        };
        controller.send(0x21, InputMsg::Event(down));
        controller.send(0x21, pointer(0.9, 0.1));
        controller.send(0x21, InputMsg::Event(up));
        assert_eq!(recv_event(&rx1), down);
        let mut saw_position = false;
        recv_matching(&rx1, |m| {
            match m {
                InputMsg::Pointer(p) => saw_position = (p.x - 0.9).abs() < 0.001,
                InputMsg::Event(e) if e == up => {
                    assert!(saw_position);
                    return true;
                }
                _ => {}
            }
            false
        });
        controller.send(0x21, InputMsg::Reconcile(ReconcileState::INACTIVE));
        controller.send(0x22, active());
        controller.send(0x22, key(true));
        assert_eq!(
            recv_event(&rx2),
            InputEvent::Key {
                hid: 0x04,
                down: true
            }
        );
        assert!(wait_until(|| agent2.status.snapshot().remote_active));
        assert!(wait_until(|| !agent1.status.snapshot().remote_active));
        drop(agent1);
        assert!(wait_until(|| controller
            .worker
            .status
            .snapshot()
            .devices
            .len()
            == 1));
        assert!(agent2.status.snapshot().connected);
    }

    #[test]
    fn rapid_keys_and_wheel_need_no_pointer_and_hold_survives_idle() {
        let controller = Controller::new();
        let (agent, rx) = controller.agent(0x31);
        controller.send(0x31, active());
        for _ in 0..100 {
            controller.send(0x31, key(true));
            controller.send(0x31, key(false));
        }
        for _ in 0..100 {
            assert_eq!(
                recv_event(&rx),
                InputEvent::Key {
                    hid: 0x04,
                    down: true
                }
            );
            assert_eq!(
                recv_event(&rx),
                InputEvent::Key {
                    hid: 0x04,
                    down: false
                }
            );
        }
        for delta in 1..=20 {
            controller.send(0x31, InputMsg::Event(InputEvent::Wheel { delta }));
        }
        for delta in 1..=20 {
            assert_eq!(recv_event(&rx), InputEvent::Wheel { delta });
        }
        controller.send(0x31, key(true));
        assert_eq!(
            recv_event(&rx),
            InputEvent::Key {
                hid: 0x04,
                down: true
            }
        );
        // Multiple idle snapshots: a stationary held key must not be timed out.
        let deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < deadline {
            if let Ok(message) = rx.recv_timeout(Duration::from_millis(50)) {
                match message {
                    InputMsg::Reset => panic!("held key reset while heartbeat was healthy"),
                    InputMsg::Reconcile(state) => {
                        assert!(state.active && state.keys.contains(0x04))
                    }
                    _ => {}
                }
            }
        }
        assert!(agent.status.snapshot().remote_active);
        controller.send(0x31, key(false));
        assert_eq!(
            recv_event(&rx),
            InputEvent::Key {
                hid: 0x04,
                down: false
            }
        );
    }

    #[test]
    fn disconnect_releases_and_reconnect_starts_inactive() {
        let controller = Controller::new();
        let (agent1, _rx1) = controller.agent(0x41);
        controller.send(0x41, active());
        controller.send(0x41, key(true));
        assert!(wait_until(|| agent1.status.snapshot().remote_active));
        let old_session = controller
            .layout
            .snapshot()
            .1
            .screen_spec(crate::layout::ScreenId::Device(device(0x41)))
            .unwrap()
            .session;
        // Same device opens a fresh connection while the old one is active.
        let (agent2, rx2) = controller.agent(0x41);
        recv_matching(
            &rx2,
            |m| matches!(m, InputMsg::Reconcile(s) if !s.active && s.keys.is_empty()),
        );
        assert!(!agent2.status.snapshot().remote_active);
        // Stop the old worker before its automatic retry can replace agent2.
        drop(agent1);
        assert!(agent2.status.snapshot().connected);
        // A queued capture from the previous connection must never reactivate
        // or type into the new connection, even though device identity matches.
        for input in [active(), key(true)] {
            controller
                .tx
                .send(CaptureMsg {
                    device: device(0x41),
                    session: old_session,
                    input,
                })
                .unwrap();
        }
        controller.send(0x41, InputMsg::Event(InputEvent::Wheel { delta: 1 }));
        assert_eq!(recv_event(&rx2), InputEvent::Wheel { delta: 1 });
        assert!(!agent2.status.snapshot().remote_active);
        drop(controller);
        recv_matching(&rx2, |m| matches!(m, InputMsg::Reset));
        assert!(wait_until(|| !agent2.status.snapshot().connected));
    }

    #[test]
    fn wrong_version_and_pairing_code_never_become_online() {
        let controller = Controller::new();
        let invite = Invite::decode(&controller.code).unwrap();
        let address = SocketAddr::from((invite.ip, invite.port));
        let socket = TcpStream::connect(address).unwrap();
        let mut wire = FramedStream::new(socket, Status::default()).unwrap();
        let mut packet = vec![0; protocol::HELLO_LEN];
        packet[..4].copy_from_slice(protocol::MAGIC);
        packet[4] = protocol::VERSION + 1;
        wire.queue(packet).unwrap();
        wire.flush().unwrap();
        assert!(wait_until(|| wire.read_packet().is_err()));
        assert!(controller.worker.status.snapshot().devices.is_empty());

        let wrong = Invite::generate(invite.ip, invite.port).encode();
        let stop = AtomicBool::new(false);
        assert!(connect_agent(
            address,
            &wrong,
            ScreenInfo {
                width: 1920,
                height: 1080
            },
            device(0x51),
            &stop,
            &Status::default()
        )
        .is_err());
        assert!(controller.worker.status.snapshot().devices.is_empty());
    }

    #[test]
    fn pairing_code_reuses_tcp_port_and_pacing_stays_bounded() {
        let (listener, code) = bind_listener(Ipv4Addr::LOCALHOST, "").unwrap();
        drop(listener);
        let (_listener, reused) = bind_listener(Ipv4Addr::LOCALHOST, &code).unwrap();
        assert_eq!(code, reused);
        let mut rtt = RttEstimator::new();
        assert_eq!(pointer_interval(&rtt), INPUT_INTERVAL);
        rtt.observe(100.0);
        assert_eq!(pointer_interval(&rtt), SLOW_INPUT_INTERVAL);
        for _ in 0..100 {
            rtt.observe(1.0);
        }
        assert_eq!(pointer_interval(&rtt), INPUT_INTERVAL);
    }
}
