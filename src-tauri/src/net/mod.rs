mod metrics;
pub mod protocol;

use crate::config::{AppConfig, ConfigHandle};
use crate::error;
use crate::input::{
    self, adapt_event, adapt_reconcile, CaptureMsg, InjectMsg, InputEvent, ReconcileState,
    RemoteInput, WarpInput,
};
use crate::invite::{self, Invite, PORT_RANGE_END, PORT_RANGE_START};
use crate::layout::{
    default_placement, device_id_to_hex, DeviceId, DevicePlacement, LayoutStore, WorldLayout,
};
use crate::status::{PeerStatus, ScreenInfo, Status};
use crossbeam_channel::{bounded, never, tick, Receiver, Sender, TrySendError};
use metrics::{LossTracker, RttEstimator};
use protocol::{
    build_pake_accept, build_pake_hello, decode_event, decode_input, decode_pong, decode_reconcile,
    decode_u64, decode_warp, derive_session_keys, encode_event, encode_input, encode_pong,
    encode_reconcile, encode_u64, encode_warp, packet_sid, parse_pake_accept, parse_pake_hello,
    random_sid, transcript_hash, Handshake, ProtocolError, Session, Side, CONFIRM_PAYLOAD,
    HELLO_LEN, TYPE_ACCEPT, TYPE_BYE, TYPE_CONFIRM, TYPE_CONFIRM_ACK, TYPE_EVENT, TYPE_HELLO,
    TYPE_INPUT, TYPE_PING, TYPE_PONG, TYPE_RECONCILE, TYPE_WARP,
};
use rand::RngCore;
use serde::Serialize;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const HANDSHAKE_RETRY: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIRM_RETRY: Duration = Duration::from_millis(500);
const CONFIRM_TIMEOUT: Duration = Duration::from_millis(2_500);
const PING_ACTIVE_INTERVAL: Duration = Duration::from_millis(100);
const PING_IDLE_INTERVAL: Duration = Duration::from_millis(500);
const RECONCILE_INTERVAL: Duration = Duration::from_millis(500);
const PONG_TIMEOUT: Duration = Duration::from_millis(2_500);
const CONTROLLER_READER_TIMEOUT: Duration = Duration::from_millis(50);
const CONTROLLER_NET_QUEUE: usize = 512;
const AGENT_SESSION_TIMEOUT: Duration = Duration::from_millis(3_000);
const AGENT_ACTIVE_TIMEOUT: Duration = Duration::from_secs(1);
const PENDING_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_READ_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_PEERS: usize = 8;
const MAX_PENDING: usize = 8;

const INPUT_INTERVAL: Duration = Duration::from_millis(2);
const DSCP_EF: u32 = 0xB8;
const PAKE_ID_CONTROLLER: &[u8] = b"GlideLink v1 controller";
const PAKE_ID_AGENT: &[u8] = b"GlideLink v1 agent";

type Datagram = (Vec<u8>, SocketAddr);

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
                if v4.ip.is_loopback() || v4.ip.is_unspecified() {
                    continue;
                }
                interfaces.push(NetworkInterface {
                    name: adapter.name,
                    ip: v4.ip.to_string(),
                });
            }
        }
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name).then(a.ip.cmp(&b.ip)));
    interfaces.dedup_by(|a, b| a.name == b.name && a.ip == b.ip);
    interfaces
}

fn interface_name_for_ip(ip: Ipv4Addr) -> Option<String> {
    local_interfaces()
        .into_iter()
        .find(|interface| interface.ip == ip.to_string())
        .map(|interface| interface.name)
}

fn set_dscp(socket: &UdpSocket) {
    if let Err(error) = socket2::SockRef::from(socket).set_tos_v4(DSCP_EF) {
        log::debug!("DSCP EF not applied: {error}");
    }
}

fn spawn_reader(
    socket: &UdpSocket,
    tx: Sender<Datagram>,
    stop: Arc<AtomicBool>,
    drops: Arc<AtomicU64>,
) -> Result<JoinHandle<()>, String> {
    let socket = socket
        .try_clone()
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;
    socket
        .set_read_timeout(Some(CONTROLLER_READER_TIMEOUT))
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;

    thread::Builder::new()
        .name("glidelink-controller-rx".into())
        .spawn(move || {
            crate::sched::boost_current_thread();
            let mut buf = [0u8; 2048];
            while !stop.load(Ordering::Acquire) {
                match socket.recv_from(&mut buf) {
                    Ok((n, from)) => match tx.try_send((buf[..n].to_vec(), from)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            drops.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    },
                    Err(e) if is_benign_recv_error(&e) => {}
                    Err(e) => {
                        log::warn!("UDP receive error: {e}");
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
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
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct InputPacer {
    interval: Duration,
    last_move_send: Instant,
    rtt: RttEstimator,
}

impl InputPacer {
    fn new() -> Self {
        Self {
            interval: INPUT_INTERVAL,
            last_move_send: Instant::now()
                .checked_sub(INPUT_INTERVAL)
                .unwrap_or_else(Instant::now),
            rtt: RttEstimator::new(),
        }
    }

    fn observe_rtt(&mut self, rtt_ms: f64) -> f64 {
        self.rtt.observe(rtt_ms);
        self.rtt.jitter_ms()
    }
}

/// Windows reports ICMP port-unreachable as `WSAECONNRESET` on an unconnected
/// UDP socket. Sending to an agent that is not running yet must not be treated
/// as a fatal receive error or spam the log at the handshake rate.
fn is_benign_recv_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionRefused
            | ErrorKind::Interrupted
    )
}

fn send_packet(
    socket: &UdpSocket,
    packet: &[u8],
    addr: SocketAddr,
    status: &Status,
) -> std::io::Result<usize> {
    let sent = socket.send_to(packet, addr)?;
    status.add_sent(sent);
    Ok(sent)
}

fn bind_free_port(ip: Ipv4Addr) -> Result<UdpSocket, String> {
    let count = u32::from(PORT_RANGE_END - PORT_RANGE_START + 1);
    let offset = rand::rngs::OsRng.next_u32() % count;
    for step in 0..count {
        let candidate = PORT_RANGE_START + ((offset + step) % count) as u16;
        let addr = SocketAddr::new(IpAddr::V4(ip), candidate);
        match UdpSocket::bind(addr) {
            Ok(socket) => return Ok(socket),
            Err(e) if e.kind() == ErrorKind::AddrInUse => continue,
            Err(e) => return Err(error::detail("E_NET_BIND", format!("{addr}: {e}"))),
        }
    }
    Err(error::code("E_PORT_RANGE"))
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

fn bind_listener(ip: Ipv4Addr, last_code: &str) -> Result<(UdpSocket, String), String> {
    if let Ok(invite) = Invite::decode(last_code) {
        if invite.ip == ip {
            let addr = SocketAddr::new(IpAddr::V4(ip), invite.port);
            if let Ok(socket) = UdpSocket::bind(addr) {
                return Ok((socket, last_code.to_string()));
            }
        }
    }
    let socket = bind_free_port(ip)?;
    let port = socket
        .local_addr()
        .map_err(|e| error::detail("E_NET_BIND", e))?
        .port();
    Ok((socket, Invite::generate(ip, port).encode()))
}

fn send_encrypted(
    socket: &UdpSocket,
    session: &mut Session,
    kind: u8,
    payload: &[u8],
    addr: SocketAddr,
    status: &Status,
) -> Result<(), String> {
    let packet = session.encrypt_packet(kind, payload);
    send_packet(socket, &packet, addr, status)
        .map(|_| ())
        .map_err(|e| e.to_string())
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
        InputEvent::CapsLock { locked } => {
            state.caps_lock = locked;
        }
        InputEvent::ReleaseAll => {
            state.buttons = 0;
            state.keys = crate::input::keymap::KeyState::EMPTY;
        }
        InputEvent::Wheel { .. } => {}
    }
}

fn peer_reconcile(peer: &mut PeerSession, active: bool, adapt: bool) -> ReconcileState {
    let mut state = peer.reconcile;
    state.active = active;
    if !active {
        state.buttons = 0;
        state.keys = crate::input::keymap::KeyState::EMPTY;
    }
    if adapt {
        adapt_reconcile(&mut state);
    }
    peer.last_reconcile_send = Instant::now();
    state
}

fn scale_relative(input: RemoteInput, host: ScreenInfo, peer: ScreenInfo) -> RemoteInput {
    if host.width == 0 || host.height == 0 {
        return input;
    }
    let dx = (input.dx as f64 * peer.width as f64 / host.width as f64).round() as i32;
    let dy = (input.dy as f64 * peer.height as f64 / host.height as f64).round() as i32;
    RemoteInput { dx, dy }
}

fn flush_peer_move(
    socket: &UdpSocket,
    peer: &mut PeerSession,
    host: ScreenInfo,
    status: &Status,
) -> Result<(), String> {
    let Some(input) = peer.pending_move.take() else {
        return Ok(());
    };
    let input = scale_relative(input, host, peer.screen);
    send_encrypted(
        socket,
        &mut peer.session,
        TYPE_INPUT,
        &encode_input(&input),
        peer.addr,
        status,
    )?;
    peer.pacer.last_move_send = Instant::now();
    Ok(())
}

struct PendingAccept {
    sid: [u8; protocol::SID_LEN],
    addr: SocketAddr,
    session: Session,
    accept_packet: Vec<u8>,
    device_id: DeviceId,
    screen: ScreenInfo,
    platform: u8,
    created: Instant,
}

struct PeerSession {
    device_id: DeviceId,
    sid: [u8; protocol::SID_LEN],
    addr: SocketAddr,
    session: Session,
    screen: ScreenInfo,
    platform: u8,
    pacer: InputPacer,
    pending_move: Option<RemoteInput>,
    reconcile: ReconcileState,
    last_ping: Instant,
    last_pong: Instant,
    pending_pings: Vec<(u64, Instant)>,
    next_ping_id: u64,
    last_reconcile_send: Instant,
    latency_ms: Option<f64>,
    jitter_ms: Option<f64>,
    loss_percent: Option<f64>,
}

struct Hub {
    socket: UdpSocket,
    pairing_code: String,
    local_screen: ScreenInfo,
    local_device: DeviceId,
    adapt_modifiers: bool,
    config: ConfigHandle,
    layout: LayoutStore,
    status: Status,
    peers: HashMap<DeviceId, PeerSession>,
    sid_index: HashMap<[u8; protocol::SID_LEN], DeviceId>,
    pending: HashMap<[u8; protocol::SID_LEN], PendingAccept>,
    active_target: Option<DeviceId>,
    limiters: HashMap<IpAddr, PakeLimiter>,
    last_status_refresh: Instant,
}

impl Hub {
    fn handle_capture(&mut self, message: CaptureMsg) {
        match message {
            CaptureMsg::Move { device, input } => {
                if let Some(peer) = self.peers.get_mut(&device) {
                    let pending = peer.pending_move.get_or_insert_with(RemoteInput::default);
                    pending.dx = pending.dx.saturating_add(input.dx);
                    pending.dy = pending.dy.saturating_add(input.dy);
                }
            }
            CaptureMsg::Warp { device, input } => {
                let host = self.local_screen;
                if let Some(peer) = self.peers.get_mut(&device) {
                    if let Err(e) = flush_peer_move(&self.socket, peer, host, &self.status) {
                        log::warn!("input move send failed: {e}");
                    }
                    if let Err(e) = send_encrypted(
                        &self.socket,
                        &mut peer.session,
                        TYPE_WARP,
                        &encode_warp(&input),
                        peer.addr,
                        &self.status,
                    ) {
                        log::warn!("warp send failed: {e}");
                    }
                }
            }
            CaptureMsg::Event { device, event } => {
                let adapt = self.adapt_modifiers
                    && self
                        .peers
                        .get(&device)
                        .map(|peer| peer.platform != input::platform_id())
                        .unwrap_or(false);
                if let Some(peer) = self.peers.get_mut(&device) {
                    apply_event_to_reconcile(&mut peer.reconcile, event);
                    if let Err(e) =
                        flush_peer_move(&self.socket, peer, self.local_screen, &self.status)
                    {
                        log::warn!("input move send failed: {e}");
                    }
                    let mut event = event;
                    if adapt {
                        adapt_event(&mut event);
                    }
                    if let Err(e) = send_encrypted(
                        &self.socket,
                        &mut peer.session,
                        TYPE_EVENT,
                        &encode_event(&event),
                        peer.addr,
                        &self.status,
                    ) {
                        log::warn!("event send failed: {e}");
                    }
                }
            }
            CaptureMsg::Reconcile { device, state } => {
                if !self.peers.contains_key(&device) {
                    return;
                }
                if state.active {
                    if self.active_target != Some(device) {
                        if let Some(old) = self.active_target {
                            self.release_peer(old);
                        }
                        self.active_target = Some(device);
                    }
                } else if self.active_target == Some(device) {
                    self.active_target = None;
                }

                let adapt = self.adapt_modifiers
                    && self
                        .peers
                        .get(&device)
                        .map(|peer| peer.platform != input::platform_id())
                        .unwrap_or(false);
                if let Some(peer) = self.peers.get_mut(&device) {
                    peer.reconcile = state;
                    if let Err(e) =
                        flush_peer_move(&self.socket, peer, self.local_screen, &self.status)
                    {
                        log::warn!("input move send failed: {e}");
                    }
                    let state = peer_reconcile(peer, state.active, adapt);
                    if let Err(e) = send_encrypted(
                        &self.socket,
                        &mut peer.session,
                        TYPE_RECONCILE,
                        &encode_reconcile(&state),
                        peer.addr,
                        &self.status,
                    ) {
                        log::warn!("reconcile send failed: {e}");
                    }
                }
                self.status.update(|s| {
                    s.remote_active = self.active_target.is_some();
                    s.target_device = self.active_target.map(|id| device_id_to_hex(&id));
                });
                self.refresh_status();
            }
        }
    }

    fn release_peer(&mut self, device: DeviceId) {
        let Some(peer) = self.peers.get_mut(&device) else {
            return;
        };
        peer.reconcile.active = false;
        peer.reconcile.buttons = 0;
        peer.reconcile.keys = crate::input::keymap::KeyState::EMPTY;
        peer.pending_move = None;
        let _ = send_encrypted(
            &self.socket,
            &mut peer.session,
            TYPE_EVENT,
            &encode_event(&InputEvent::ReleaseAll),
            peer.addr,
            &self.status,
        );
        let state = peer_reconcile(peer, false, false);
        let _ = send_encrypted(
            &self.socket,
            &mut peer.session,
            TYPE_RECONCILE,
            &encode_reconcile(&state),
            peer.addr,
            &self.status,
        );
    }

    fn handle_hello(&mut self, packet: &[u8], from: SocketAddr) {
        let Ok((hello, pake_msg)) = parse_pake_hello(packet) else {
            return;
        };
        if let Some(pending) = self.pending.get(&hello.sid) {
            if pending.addr == from {
                let _ = send_packet(&self.socket, &pending.accept_packet, from, &self.status);
            }
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        if self.peers.len() >= MAX_PEERS && !self.peers.contains_key(&hello.device_id) {
            return;
        }
        {
            let limiter = self
                .limiters
                .entry(from.ip())
                .or_insert_with(PakeLimiter::new);
            if !limiter.allow() {
                return;
            }
        }
        let (state, accept_msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(self.pairing_code.as_bytes()),
            &Identity::new(PAKE_ID_CONTROLLER),
            &Identity::new(PAKE_ID_AGENT),
        );
        let shared = match state.finish(&pake_msg) {
            Ok(key) => key,
            Err(_) => {
                log::debug!("PAKE exchange failed for {from}");
                return;
            }
        };
        let accept = Handshake {
            sid: hello.sid,
            screen: self.local_screen,
            timestamp_ms: protocol::now_ms(),
            platform: input::platform_id(),
            device_id: self.local_device,
        };
        let accept_packet = build_pake_accept(&accept, &accept_msg);
        let transcript = transcript_hash(packet, &accept_packet);
        let (c2a, a2c) = derive_session_keys(&shared, &hello.sid, &transcript);
        let session = Session::from_keys(&c2a, &a2c, hello.sid, Side::Controller);
        if let Err(e) = send_packet(&self.socket, &accept_packet, from, &self.status) {
            log::warn!("accept send failed: {e}");
            return;
        }
        self.pending.insert(
            hello.sid,
            PendingAccept {
                sid: hello.sid,
                addr: from,
                session,
                accept_packet,
                device_id: hello.device_id,
                screen: hello.screen,
                platform: hello.platform,
                created: Instant::now(),
            },
        );
    }

    fn handle_pending_confirm(
        &mut self,
        sid: [u8; protocol::SID_LEN],
        packet: &[u8],
        from: SocketAddr,
    ) {
        let Some(mut pending) = self.pending.remove(&sid) else {
            return;
        };
        if pending.addr != from {
            self.pending.insert(sid, pending);
            return;
        }
        match pending.session.decrypt_packet_with_seq(packet) {
            Ok((TYPE_CONFIRM, payload, _seq)) if payload == CONFIRM_PAYLOAD => {
                let ack = pending
                    .session
                    .encrypt_packet(TYPE_CONFIRM_ACK, CONFIRM_PAYLOAD);
                let _ = send_packet(&self.socket, &ack, from, &self.status);
                self.promote(pending);
            }
            Ok(_) => {
                self.pending.insert(sid, pending);
            }
            Err(e) => {
                log::debug!("confirm from {from} rejected: {e}");
                self.pending.insert(sid, pending);
            }
        }
    }

    fn promote(&mut self, pending: PendingAccept) {
        let device = pending.device_id;
        let old = self.peers.remove(&device);
        let (old_reconcile, old_move) = if let Some(mut old) = old {
            self.sid_index.remove(&old.sid);
            let _ = send_encrypted(
                &self.socket,
                &mut old.session,
                TYPE_BYE,
                &[],
                old.addr,
                &self.status,
            );
            (Some(old.reconcile), old.pending_move)
        } else {
            (None, None)
        };

        let now = Instant::now();
        let reconcile = old_reconcile.unwrap_or_else(|| {
            let mut state = ReconcileState::INACTIVE;
            state.active = self.active_target == Some(device);
            state
        });
        self.peers.insert(
            device,
            PeerSession {
                device_id: device,
                sid: pending.sid,
                addr: pending.addr,
                session: pending.session,
                screen: pending.screen,
                platform: pending.platform,
                pacer: InputPacer::new(),
                pending_move: old_move,
                reconcile,
                last_ping: now.checked_sub(PING_IDLE_INTERVAL).unwrap_or(now),
                last_pong: now,
                pending_pings: Vec::new(),
                next_ping_id: 1,
                last_reconcile_send: now.checked_sub(RECONCILE_INTERVAL).unwrap_or(now),
                latency_ms: None,
                jitter_ms: None,
                loss_percent: None,
            },
        );
        self.sid_index.insert(pending.sid, device);
        self.ensure_placement(device, pending.platform, pending.addr, pending.screen);
        self.refresh_layout();

        let adapt = self.adapt_modifiers && pending.platform != input::platform_id();
        if let Some(peer) = self.peers.get_mut(&device) {
            let host = self.local_screen;
            let _ = flush_peer_move(&self.socket, peer, host, &self.status);
            let active = self.active_target == Some(device);
            let state = peer_reconcile(peer, active, adapt);
            let _ = send_encrypted(
                &self.socket,
                &mut peer.session,
                TYPE_RECONCILE,
                &encode_reconcile(&state),
                peer.addr,
                &self.status,
            );
        }
        log::info!(
            "agent {} connected ({}x{}, {})",
            peer_label(pending.platform, pending.addr),
            pending.screen.width,
            pending.screen.height,
            device_id_to_hex(&device)
        );
        self.refresh_status();
    }

    fn handle_peer_packet(&mut self, device: DeviceId, packet: &[u8], from: SocketAddr) {
        let remove = {
            let Some(peer) = self.peers.get_mut(&device) else {
                return;
            };
            if peer.addr != from {
                return;
            }
            match peer.session.decrypt_packet_with_seq(packet) {
                Ok((kind, payload, _seq)) => match kind {
                    TYPE_PONG => {
                        peer.last_pong = Instant::now();
                        if let Ok((id, stats)) = decode_pong(&payload) {
                            if let Some(index) = peer
                                .pending_pings
                                .iter()
                                .position(|(pending_id, _)| *pending_id == id)
                            {
                                let (_, sent_at) = peer.pending_pings.remove(index);
                                let rtt = sent_at.elapsed().as_secs_f64() * 1000.0;
                                peer.latency_ms = Some(rtt);
                                peer.jitter_ms = Some(peer.pacer.observe_rtt(rtt));
                            }
                            if let Some((expected, received)) = stats {
                                peer.loss_percent = if expected == 0 {
                                    None
                                } else {
                                    Some(
                                        ((expected.saturating_sub(received) as f64
                                            / expected as f64)
                                            * 100.0)
                                            .clamp(0.0, 100.0),
                                    )
                                };
                            }
                        }
                        false
                    }
                    TYPE_CONFIRM => {
                        if payload == CONFIRM_PAYLOAD {
                            let ack = peer
                                .session
                                .encrypt_packet(TYPE_CONFIRM_ACK, CONFIRM_PAYLOAD);
                            let _ = send_packet(&self.socket, &ack, from, &self.status);
                        }
                        false
                    }
                    TYPE_BYE => true,
                    _ => false,
                },
                Err(ProtocolError::Replay) => false,
                Err(e) => {
                    log::debug!("packet from {from} rejected: {e}");
                    false
                }
            }
        };
        if remove {
            self.remove_peer(device);
        }
        self.refresh_status();
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
        let online: Vec<DeviceId> = self.peers.keys().copied().collect();
        let world =
            crate::layout::world_from_placements(self.local_screen, &config.layout, &online);
        self.layout.replace(world);
    }

    fn remove_peer(&mut self, device: DeviceId) {
        let Some(peer) = self.peers.remove(&device) else {
            return;
        };
        self.sid_index.remove(&peer.sid);
        if self.active_target == Some(device) {
            self.active_target = None;
        }
        self.refresh_layout();
        log::info!(
            "agent {} disconnected",
            peer_label(peer.platform, peer.addr)
        );
        self.refresh_status();
    }

    fn refresh_status(&mut self) {
        let now = Instant::now();
        self.last_status_refresh = now;
        let mut devices: Vec<PeerStatus> = self
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
                latency_ms: peer.latency_ms,
                jitter_ms: peer.jitter_ms,
                packet_loss_percent: peer.loss_percent,
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
            s.latency_ms = active.and_then(|p| p.latency_ms);
            s.jitter_ms = active.and_then(|p| p.jitter_ms);
            s.packet_loss_percent = active.and_then(|p| p.loss_percent);
            s.input_interval_ms = active.map(|p| p.pacer.interval.as_secs_f64() * 1000.0);
        });
    }

    fn on_timer(&mut self, now: Instant) {
        self.pending
            .retain(|_, pending| now.saturating_duration_since(pending.created) < PENDING_TIMEOUT);
        self.limiters.retain(|_, limiter| limiter.active());

        let ids: Vec<DeviceId> = self.peers.keys().copied().collect();
        let mut remove = Vec::new();
        for device in ids {
            let active = self.active_target == Some(device);
            let Some(peer) = self.peers.get_mut(&device) else {
                continue;
            };
            if now.saturating_duration_since(peer.last_pong) >= PONG_TIMEOUT {
                remove.push(device);
                continue;
            }
            let ping_interval = if active {
                PING_ACTIVE_INTERVAL
            } else {
                PING_IDLE_INTERVAL
            };
            if now.saturating_duration_since(peer.last_ping) >= ping_interval {
                let id = peer.next_ping_id;
                peer.next_ping_id = peer.next_ping_id.wrapping_add(1).max(1);
                if send_encrypted(
                    &self.socket,
                    &mut peer.session,
                    TYPE_PING,
                    &encode_u64(id),
                    peer.addr,
                    &self.status,
                )
                .is_ok()
                {
                    peer.pending_pings.push((id, now));
                    if peer.pending_pings.len() > 32 {
                        peer.pending_pings.remove(0);
                    }
                }
                peer.last_ping = now;
            }
            if active
                && peer.pending_move.is_some()
                && now.saturating_duration_since(peer.pacer.last_move_send) >= peer.pacer.interval
            {
                if let Err(e) = flush_peer_move(&self.socket, peer, self.local_screen, &self.status)
                {
                    log::warn!("input send failed: {e}");
                }
            }
            if !active {
                peer.pending_move = None;
            }
            if now.saturating_duration_since(peer.last_reconcile_send) >= RECONCILE_INTERVAL {
                let adapt = self.adapt_modifiers && peer.platform != input::platform_id();
                let state = peer_reconcile(peer, active, adapt);
                let _ = send_encrypted(
                    &self.socket,
                    &mut peer.session,
                    TYPE_RECONCILE,
                    &encode_reconcile(&state),
                    peer.addr,
                    &self.status,
                );
            }
        }
        for device in remove {
            self.remove_peer(device);
        }
        if now.saturating_duration_since(self.last_status_refresh) >= Duration::from_millis(500) {
            self.refresh_status();
        }
    }

    fn shutdown(&mut self) {
        for peer in self.peers.values_mut() {
            let _ = send_encrypted(
                &self.socket,
                &mut peer.session,
                TYPE_BYE,
                &[],
                peer.addr,
                &self.status,
            );
        }
    }
}

impl PakeLimiter {
    fn active(&self) -> bool {
        self.last.elapsed() < Duration::from_secs(30)
    }
}

pub fn start_controller(
    config: &AppConfig,
    config_handle: ConfigHandle,
    layout: LayoutStore,
    capture_rx: Receiver<CaptureMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let ip = config.local_ipv4()?;
    let local_screen = local_screen_or_zero();
    let (socket, pairing_code) = bind_listener(ip, &config.last_invite)?;
    if let Err(e) = config_handle.update(|current| {
        current.last_invite = pairing_code.clone();
        Ok(())
    }) {
        log::warn!("could not persist listening code: {e}");
    }
    set_dscp(&socket);

    let local_device = config.device_id_bytes()?;
    match interface_name_for_ip(ip) {
        Some(name) => log::info!("controller listening on {ip} via {name}"),
        None => log::info!("controller listening on {ip}"),
    }
    let reader_drops = Arc::new(AtomicU64::new(0));
    let (net_tx, net_rx) = bounded::<Datagram>(CONTROLLER_NET_QUEUE);
    let reader = spawn_reader(&socket, net_tx, Arc::clone(&stop), reader_drops)?;

    status.update(|s| {
        s.running = true;
        s.connected = false;
        s.remote_active = false;
        s.peer = None;
        s.peer_platform = None;
        s.local_screen = Some(local_screen);
        s.remote_screen = None;
        s.latency_ms = None;
        s.last_error_code = None;
        s.last_error_detail = None;
        s.pairing_code = Some(pairing_code.clone());
        s.devices.clear();
        s.target_device = None;
        s.started_at_ms = Some(protocol::now_ms());
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
    });

    let hub = Hub {
        socket,
        pairing_code: pairing_code.clone(),
        local_screen,
        local_device,
        adapt_modifiers: config.adapt_modifiers,
        config: config_handle,
        layout,
        status: status.clone(),
        peers: HashMap::new(),
        sid_index: HashMap::new(),
        pending: HashMap::new(),
        active_target: None,
        limiters: HashMap::new(),
        last_status_refresh: Instant::now(),
    };

    thread::Builder::new()
        .name("glidelink-controller-hub".into())
        .spawn(move || run_hub(hub, capture_rx, net_rx, stop, reader))
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

fn run_hub(
    mut hub: Hub,
    capture_rx: Receiver<CaptureMsg>,
    net_rx: Receiver<Datagram>,
    stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
) {
    crate::sched::boost_current_thread();
    let _session_guard = crate::sched::SessionGuard::begin();
    let mut capture_source = capture_rx.clone();
    let mut net_source = net_rx.clone();
    let tick = tick(Duration::from_millis(1));

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        crossbeam_channel::select! {
            recv(capture_source) -> message => {
                match message {
                    Ok(message) => {
                        hub.handle_capture(message);
                        while let Ok(message) = capture_rx.try_recv() {
                            hub.handle_capture(message);
                        }
                    }
                    Err(_) => capture_source = never(),
                }
            }
            recv(net_source) -> datagram => {
                match datagram {
                    Ok((packet, from)) => {
                        hub.status.add_received(packet.len());
                        if packet.len() == HELLO_LEN && packet.get(5) == Some(&TYPE_HELLO) {
                            hub.handle_hello(&packet, from);
                        } else if let Ok(sid) = packet_sid(&packet) {
                            if let Some(device) = hub.sid_index.get(&sid).copied() {
                                hub.handle_peer_packet(device, &packet, from);
                            } else if hub.pending.contains_key(&sid) {
                                hub.handle_pending_confirm(sid, &packet, from);
                            }
                        }
                        while let Ok((packet, from)) = net_rx.try_recv() {
                            hub.status.add_received(packet.len());
                            if packet.len() == HELLO_LEN && packet.get(5) == Some(&TYPE_HELLO) {
                                hub.handle_hello(&packet, from);
                            } else if let Ok(sid) = packet_sid(&packet) {
                                if let Some(device) = hub.sid_index.get(&sid).copied() {
                                    hub.handle_peer_packet(device, &packet, from);
                                } else if hub.pending.contains_key(&sid) {
                                    hub.handle_pending_confirm(sid, &packet, from);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        log::warn!("controller receive thread stopped");
                        net_source = never();
                    }
                }
            }
            recv(tick) -> _ => {}
        }
        hub.on_timer(Instant::now());
    }

    hub.shutdown();
    hub.layout.update(|layout| {
        for screen in layout.screens.iter_mut() {
            screen.online = false;
        }
        true
    });
    if reader.join().is_err() {
        log::error!("controller receive thread panicked");
    }
    hub.status.update(|s| {
        s.connected = false;
        s.remote_active = false;
        s.peer = None;
        s.devices.clear();
        s.target_device = None;
    });
}

struct AgentHandshake {
    started: Instant,
    last_send: Instant,
}

struct AgentSession {
    session: Session,
    last_packet: Instant,
    loss: LossTracker,
    last_semantic_seq: u64,
    last_move_seq: u64,
}

pub fn start_agent(
    config: &AppConfig,
    pairing_code: &str,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let invite = Invite::decode(pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    let controller = SocketAddr::new(IpAddr::V4(invite.ip), invite.port);
    let pairing_code = invite::normalize_code(pairing_code);
    let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let socket =
        UdpSocket::bind(SocketAddr::new(bind_ip, 0)).map_err(|e| error::detail("E_NET_BIND", e))?;
    socket
        .set_read_timeout(Some(AGENT_READ_TIMEOUT))
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;
    set_dscp(&socket);
    let local_screen = local_screen_or_zero();
    let local_device = config.device_id_bytes()?;

    status.update(|s| {
        s.running = true;
        s.connected = false;
        s.remote_active = false;
        s.latency_ms = None;
        s.local_screen = Some(local_screen);
        s.last_error_code = None;
        s.last_error_detail = None;
        s.peer = Some(controller.to_string());
        s.remote_screen = None;
        s.started_at_ms = Some(protocol::now_ms());
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.pairing_code = Some(pairing_code.clone());
        s.devices.clear();
        s.target_device = None;
    });

    thread::Builder::new()
        .name("glidelink-agent-connect".into())
        .spawn(move || {
            crate::sched::boost_current_thread();
            let _guard = crate::sched::SessionGuard::begin();
            agent_loop(
                socket,
                controller,
                pairing_code,
                local_screen,
                local_device,
                inject_tx,
                stop,
                status,
            );
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

fn agent_loop(
    socket: UdpSocket,
    controller: SocketAddr,
    pairing_code: String,
    local_screen: ScreenInfo,
    local_device: DeviceId,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) {
    let mut buf = [0u8; 2048];
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let result = agent_attempt(
            &socket,
            &mut buf,
            controller,
            &pairing_code,
            local_screen,
            local_device,
            &inject_tx,
            &stop,
            &status,
        );
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Err(e) = result {
            log::debug!("agent connection attempt ended: {e}");
        }
        let _ = inject_tx.send(InjectMsg::Reset);
        status.update(|s| {
            s.connected = false;
            s.remote_active = false;
            s.latency_ms = None;
            s.devices.clear();
            s.target_device = None;
        });
        thread::sleep(Duration::from_millis(250));
    }
    let _ = inject_tx.send(InjectMsg::Reset);
}

fn agent_attempt(
    socket: &UdpSocket,
    buf: &mut [u8],
    controller: SocketAddr,
    pairing_code: &str,
    local_screen: ScreenInfo,
    local_device: DeviceId,
    inject_tx: &Sender<InjectMsg>,
    stop: &Arc<AtomicBool>,
    status: &Status,
) -> Result<(), String> {
    let sid = random_sid();
    let (state, hello_msg) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pairing_code.as_bytes()),
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
    send_packet(socket, &hello_packet, controller, status)
        .map_err(|e| error::detail("E_NET_SEND", e))?;
    let mut handshake = AgentHandshake {
        started: Instant::now(),
        last_send: Instant::now(),
    };

    let mut state = Some(state);
    let (accept, session) = loop {
        if stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        if Instant::now().saturating_duration_since(handshake.started) >= HANDSHAKE_TIMEOUT {
            status.set_error("NET_NO_RESPONSE", None);
            return Err(error::code("NET_NO_RESPONSE"));
        }
        if Instant::now().saturating_duration_since(handshake.last_send) >= HANDSHAKE_RETRY {
            let _ = send_packet(socket, &hello_packet, controller, status);
            handshake.last_send = Instant::now();
        }
        match socket.recv_from(buf) {
            Ok((n, from)) if from == controller => {
                if n != protocol::ACCEPT_LEN || buf.get(5) != Some(&TYPE_ACCEPT) {
                    continue;
                }
                let Ok((accept_handshake, accept_msg)) = parse_pake_accept(&buf[..n], &sid) else {
                    continue;
                };
                let Some(state) = state.take() else {
                    continue;
                };
                let shared = state
                    .finish(&accept_msg)
                    .map_err(|_| error::code("E_PAKE"))?;
                let transcript = transcript_hash(&hello_packet, &buf[..n]);
                let (c2a, a2c) = derive_session_keys(&shared, &sid, &transcript);
                let session = Session::from_keys(&c2a, &a2c, sid, Side::Agent);
                break (accept_handshake, session);
            }
            Ok(_) => {}
            Err(e) if is_benign_recv_error(&e) => {}
            Err(e) => return Err(error::detail("E_NET_RECV", e)),
        }
    };

    let mut session = session;
    let mut last_confirm = Instant::now()
        .checked_sub(CONFIRM_RETRY)
        .unwrap_or_else(Instant::now);
    let confirm_started = Instant::now();
    let ack_seq = loop {
        if stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        if Instant::now().saturating_duration_since(confirm_started) >= CONFIRM_TIMEOUT {
            status.set_error("NET_TIMEOUT", None);
            return Err(error::code("NET_TIMEOUT"));
        }
        if Instant::now().saturating_duration_since(last_confirm) >= CONFIRM_RETRY {
            let packet = session.encrypt_packet(TYPE_CONFIRM, CONFIRM_PAYLOAD);
            send_packet(socket, &packet, controller, status)
                .map_err(|e| error::detail("E_NET_SEND", e))?;
            last_confirm = Instant::now();
        }
        match socket.recv_from(buf) {
            Ok((n, from)) if from == controller => {
                match session.decrypt_packet_with_seq(&buf[..n]) {
                    Ok((TYPE_CONFIRM_ACK, payload, seq)) if payload == CONFIRM_PAYLOAD => {
                        break seq
                    }
                    Ok(_) | Err(ProtocolError::Replay) => {}
                    Err(e) => log::debug!("confirm ack rejected: {e}"),
                }
            }
            Ok(_) => {}
            Err(e) if is_benign_recv_error(&e) => {}
            Err(e) => return Err(error::detail("E_NET_RECV", e)),
        }
    };

    let label = peer_label(accept.platform, controller);
    status.update(|s| {
        s.connected = true;
        s.remote_active = false;
        s.peer = Some(controller.to_string());
        s.peer_platform = Some(accept.platform);
        s.remote_screen = Some(accept.screen);
        s.last_error_code = None;
        s.last_error_detail = None;
        s.devices = vec![PeerStatus {
            device_id: device_id_to_hex(&accept.device_id),
            label,
            addr: controller.to_string(),
            platform: Some(accept.platform),
            screen: Some(accept.screen),
            online: true,
            active: false,
            latency_ms: None,
            jitter_ms: None,
            packet_loss_percent: None,
            last_error_code: None,
        }];
    });
    let _ = inject_tx.send(InjectMsg::Reset);

    let mut active_session = AgentSession {
        session,
        last_packet: Instant::now(),
        loss: LossTracker::new(ack_seq),
        last_semantic_seq: 0,
        last_move_seq: 0,
    };
    let mut last_input = Instant::now();
    let mut active_reported = false;

    loop {
        if stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        if Instant::now().saturating_duration_since(active_session.last_packet)
            >= AGENT_SESSION_TIMEOUT
        {
            return Err(error::code("NET_PEER_DISCONNECTED"));
        }
        if active_reported
            && Instant::now().saturating_duration_since(last_input) >= AGENT_ACTIVE_TIMEOUT
        {
            active_reported = false;
            let _ = inject_tx.send(InjectMsg::Event(InputEvent::ReleaseAll));
            status.update(|s| s.remote_active = false);
        }
        match socket.recv_from(buf) {
            Ok((n, from)) if from == controller => {
                let Ok((kind, payload, seq)) =
                    active_session.session.decrypt_packet_with_seq(&buf[..n])
                else {
                    continue;
                };
                active_session.loss.observe(seq);
                match kind {
                    TYPE_WARP => {
                        let now = Instant::now();
                        active_session.last_packet = now;
                        if let Ok(input) = decode_warp(&payload) {
                            active_session.last_move_seq = seq;
                            let _ = inject_tx.send(InjectMsg::Warp(input));
                            last_input = now;
                            if !active_reported {
                                active_reported = true;
                                status.update(|s| s.remote_active = true);
                            }
                        }
                    }
                    TYPE_INPUT => {
                        let now = Instant::now();
                        active_session.last_packet = now;
                        if seq > active_session.last_move_seq
                            && seq > active_session.last_semantic_seq
                        {
                            if let Ok(input) = decode_input(&payload) {
                                active_session.last_move_seq = seq;
                                let _ = inject_tx.send(InjectMsg::Move(input));
                                last_input = now;
                                if !active_reported {
                                    active_reported = true;
                                    status.update(|s| s.remote_active = true);
                                }
                            }
                        }
                    }
                    TYPE_EVENT => {
                        let now = Instant::now();
                        active_session.last_packet = now;
                        if seq > active_session.last_semantic_seq {
                            if let Ok(event) = decode_event(&payload) {
                                active_session.last_semantic_seq = seq;
                                let release = matches!(event, InputEvent::ReleaseAll);
                                let _ = inject_tx.send(InjectMsg::Event(event));
                                if release {
                                    active_reported = false;
                                    status.update(|s| s.remote_active = false);
                                } else {
                                    last_input = now;
                                    if !active_reported {
                                        active_reported = true;
                                        status.update(|s| s.remote_active = true);
                                    }
                                }
                            }
                        }
                    }
                    TYPE_RECONCILE => {
                        let now = Instant::now();
                        active_session.last_packet = now;
                        if seq > active_session.last_semantic_seq {
                            if let Ok(state) = decode_reconcile(&payload) {
                                active_session.last_semantic_seq = seq;
                                let active = state.active;
                                let _ = inject_tx.send(InjectMsg::Reconcile(state));
                                if active {
                                    last_input = now;
                                    if !active_reported {
                                        active_reported = true;
                                        status.update(|s| s.remote_active = true);
                                    }
                                } else {
                                    active_reported = false;
                                    status.update(|s| s.remote_active = false);
                                }
                            }
                        }
                    }
                    TYPE_PING => {
                        active_session.last_packet = Instant::now();
                        if let Ok(id) = decode_u64(&payload) {
                            let response = active_session.session.encrypt_packet(
                                TYPE_PONG,
                                &encode_pong(
                                    id,
                                    active_session.loss.expected(),
                                    active_session.loss.received(),
                                ),
                            );
                            let _ = send_packet(socket, &response, controller, status);
                            status.update(|s| {
                                s.packet_loss_percent = active_session.loss.loss_percent()
                            });
                        }
                    }
                    TYPE_BYE => return Err(error::code("NET_PEER_DISCONNECTED")),
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(e) if is_benign_recv_error(&e) => {}
            Err(e) => return Err(error::detail("E_NET_RECV", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Role;
    use crate::input::{InputEvent, ReconcileState, RemoteInput, BUTTON_LEFT};
    use crate::layout::{device_id_to_hex, DeviceId, LayoutStore, WorldLayout};
    use crossbeam_channel::unbounded;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("glidelink-net-{}-{name}.json", std::process::id()))
    }

    fn device(byte: u8) -> DeviceId {
        [byte; 16]
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn recv_event(rx: &Receiver<InjectMsg>) -> Option<InputEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(InjectMsg::Event(event)) => return Some(event),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        None
    }

    fn recv_move(rx: &Receiver<InjectMsg>) -> Option<RemoteInput> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(InjectMsg::Move(input)) => return Some(input),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        None
    }

    fn recv_warp(rx: &Receiver<InjectMsg>) -> Option<WarpInput> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(InjectMsg::Warp(input)) => return Some(input),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        None
    }

    fn has_motion_or_event(rx: &Receiver<InjectMsg>, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(InjectMsg::Move(_)) | Ok(InjectMsg::Event(_)) => return true,
                Ok(_) => {}
                Err(_) => {}
            }
        }
        false
    }

    #[test]
    fn constants_fit_in_udp_buffers() {
        assert!(protocol::HEADER_LEN + protocol::INPUT_LEN + protocol::TAG_LEN < 2048);
        assert!(protocol::HEADER_LEN + protocol::EVENT_LEN + protocol::TAG_LEN < 2048);
        assert!(protocol::HEADER_LEN + protocol::RECONCILE_LEN + protocol::TAG_LEN < 2048);
        assert_eq!(protocol::HELLO_LEN, 88);
    }

    #[test]
    fn controller_hub_routes_two_agents_by_device() {
        let controller_stop = Arc::new(AtomicBool::new(false));
        let agent1_stop = Arc::new(AtomicBool::new(false));
        let agent2_stop = Arc::new(AtomicBool::new(false));
        let controller_status = Status::new(None);
        let agent1_status = Status::new(None);
        let agent2_status = Status::new(None);

        let controller_config = AppConfig {
            role: Role::Controller,
            local_ip: "127.0.0.1".into(),
            device_id: device_id_to_hex(&device(0x10)),
            ..AppConfig::default()
        };
        let config_path = temp_path("controller");
        let _ = std::fs::remove_file(&config_path);
        let config_handle = ConfigHandle::new(config_path.clone(), controller_config.clone());
        let host = ScreenInfo {
            width: 1920,
            height: 1080,
        };
        let layout = LayoutStore::new(WorldLayout::new(host, Vec::new()));
        let (capture_tx, capture_rx) = unbounded::<CaptureMsg>();
        let controller_handle = start_controller(
            &controller_config,
            config_handle,
            layout,
            capture_rx,
            Arc::clone(&controller_stop),
            controller_status.clone(),
        )
        .expect("start controller");
        let pairing_code = controller_status
            .snapshot()
            .pairing_code
            .expect("controller pairing code");

        let agent1_config = AppConfig {
            role: Role::Agent,
            device_id: device_id_to_hex(&device(0x21)),
            ..AppConfig::default()
        };
        let agent2_config = AppConfig {
            role: Role::Agent,
            device_id: device_id_to_hex(&device(0x22)),
            ..AppConfig::default()
        };
        let (inject1_tx, inject1_rx) = unbounded::<InjectMsg>();
        let (inject2_tx, inject2_rx) = unbounded::<InjectMsg>();
        let agent1_handle = start_agent(
            &agent1_config,
            &pairing_code,
            inject1_tx,
            Arc::clone(&agent1_stop),
            agent1_status.clone(),
        )
        .expect("start agent 1");
        let agent2_handle = start_agent(
            &agent2_config,
            &pairing_code,
            inject2_tx,
            Arc::clone(&agent2_stop),
            agent2_status.clone(),
        )
        .expect("start agent 2");

        assert!(
            wait_until(|| agent1_status.snapshot().connected
                && agent2_status.snapshot().connected
                && controller_status.snapshot().devices.len() == 2),
            "agents did not connect: controller={:?} a1={:?} a2={:?}",
            controller_status.snapshot(),
            agent1_status.snapshot(),
            agent2_status.snapshot()
        );

        let id1 = device(0x21);
        let id2 = device(0x22);
        let active = ReconcileState {
            active: true,
            caps_lock: false,
            buttons: 0,
            keys: crate::input::keymap::KeyState::EMPTY,
        };
        capture_tx
            .send(CaptureMsg::Move {
                device: id1,
                input: RemoteInput { dx: 25, dy: -75 },
            })
            .unwrap();
        capture_tx
            .send(CaptureMsg::Reconcile {
                device: id1,
                state: active,
            })
            .unwrap();
        let moved = recv_move(&inject1_rx).expect("agent 1 did not receive move");
        assert_eq!(moved.dx, 25);
        assert_eq!(moved.dy, -75);
        capture_tx
            .send(CaptureMsg::Warp {
                device: id1,
                input: WarpInput { x: 0.25, y: 0.75 },
            })
            .unwrap();
        let warp = recv_warp(&inject1_rx).expect("agent 1 did not receive warp");
        assert!((warp.x - 0.25).abs() < 0.001);
        assert!((warp.y - 0.75).abs() < 0.001);
        assert!(
            !has_motion_or_event(&inject2_rx, Duration::from_millis(150)),
            "agent 2 received agent 1 input"
        );

        capture_tx
            .send(CaptureMsg::Event {
                device: id1,
                event: InputEvent::Button {
                    button: BUTTON_LEFT,
                    down: true,
                },
            })
            .unwrap();
        assert_eq!(
            recv_event(&inject1_rx),
            Some(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true
            })
        );

        capture_tx
            .send(CaptureMsg::Reconcile {
                device: id1,
                state: ReconcileState::INACTIVE,
            })
            .unwrap();
        capture_tx
            .send(CaptureMsg::Move {
                device: id2,
                input: RemoteInput { dx: 60, dy: 40 },
            })
            .unwrap();
        capture_tx
            .send(CaptureMsg::Reconcile {
                device: id2,
                state: active,
            })
            .unwrap();
        let moved2 = recv_move(&inject2_rx).expect("agent 2 did not receive move");
        assert_eq!(moved2.dx, 60);
        assert_eq!(moved2.dy, 40);
        assert!(wait_until(
            || controller_status.snapshot().target_device == Some(device_id_to_hex(&id2))
        ));

        capture_tx
            .send(CaptureMsg::Event {
                device: id2,
                event: InputEvent::Key {
                    hid: 0x04,
                    down: true,
                },
            })
            .unwrap();
        assert_eq!(
            recv_event(&inject2_rx),
            Some(InputEvent::Key {
                hid: 0x04,
                down: true
            })
        );

        capture_tx
            .send(CaptureMsg::Move {
                device: device(0x99),
                input: RemoteInput { dx: 10, dy: 10 },
            })
            .unwrap();
        assert!(!has_motion_or_event(
            &inject1_rx,
            Duration::from_millis(150)
        ));
        assert!(!has_motion_or_event(
            &inject2_rx,
            Duration::from_millis(150)
        ));

        agent1_stop.store(true, Ordering::Release);
        let _ = agent1_handle.join();
        assert!(
            wait_until(|| controller_status.snapshot().devices.len() == 1),
            "controller did not drop agent 1: {:?}",
            controller_status.snapshot()
        );
        assert!(agent2_status.snapshot().connected);

        agent2_stop.store(true, Ordering::Release);
        controller_stop.store(true, Ordering::Release);
        let _ = agent2_handle.join();
        let _ = controller_handle.join();
        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn pairing_code_is_shared_and_reused() {
        let config = AppConfig {
            role: Role::Controller,
            local_ip: "127.0.0.1".into(),
            device_id: device_id_to_hex(&device(0x31)),
            ..AppConfig::default()
        };
        let path = temp_path("reuse");
        let _ = std::fs::remove_file(&path);
        let handle = ConfigHandle::new(path.clone(), config.clone());
        let (tx, rx) = unbounded::<CaptureMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        let status = Status::new(None);
        let host = ScreenInfo {
            width: 1920,
            height: 1080,
        };
        let layout = LayoutStore::new(WorldLayout::new(host, Vec::new()));
        let join = start_controller(
            &config,
            handle.clone(),
            layout,
            rx,
            Arc::clone(&stop),
            status.clone(),
        )
        .expect("start controller");
        let first = status.snapshot().pairing_code.expect("code");
        stop.store(true, Ordering::Release);
        let _ = join.join();
        let saved = handle.snapshot().unwrap();
        assert_eq!(saved.last_invite, first);
        let _ = tx;
        let _ = std::fs::remove_file(&path);
    }
}
