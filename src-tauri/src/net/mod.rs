mod metrics;
pub mod protocol;

use crate::config::AppConfig;
use crate::error;
use crate::input::keymap::{adapt_key_packet, KeyEvent, KeyPacket};
use crate::input::{self, CaptureMsg, CaptureShared, InjectMsg, RemoteInput};
use crate::invite::{self, Invite, PORT_RANGE_END, PORT_RANGE_START};
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::{bounded, never, tick, Receiver, Sender, TrySendError};
use metrics::{LossTracker, RttEstimator};
use protocol::{
    build_pake_accept, build_pake_hello, decode_input, decode_key, decode_pong, decode_u64,
    derive_session_keys, encode_input, encode_key, encode_pong, encode_u64, hello_sid, now_ms,
    parse_pake_accept, parse_pake_hello, random_sid, transcript_hash, Handshake, ProtocolError,
    Session, Side, CONFIRM_PAYLOAD, HELLO_LEN, PAKE_MSG_LEN, TYPE_BYE, TYPE_CONFIRM,
    TYPE_CONFIRM_ACK, TYPE_HELLO, TYPE_INPUT, TYPE_KEY, TYPE_PING, TYPE_PONG,
};
use rand::RngCore;
use serde::Serialize;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CONTROLLER_HANDSHAKE_RETRY: Duration = Duration::from_millis(500);
const CONTROLLER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROLLER_CONFIRM_RETRY: Duration = Duration::from_millis(500);
const CONTROLLER_CONFIRM_TIMEOUT: Duration = Duration::from_millis(2_500);
const CONTROLLER_PING_INTERVAL: Duration = Duration::from_millis(100);
const CONTROLLER_KEY_INTERVAL: Duration = Duration::from_millis(100);
const CONTROLLER_PONG_TIMEOUT: Duration = Duration::from_millis(2_500);
const CONTROLLER_UNREACHABLE_HINT_AFTER: Duration = Duration::from_secs(8);
const CONTROLLER_READER_TIMEOUT: Duration = Duration::from_millis(50);
const CONTROLLER_NET_QUEUE: usize = 256;
const AGENT_SESSION_TIMEOUT: Duration = Duration::from_millis(3_000);
const AGENT_ACTIVE_TIMEOUT: Duration = Duration::from_secs(1);
const AGENT_PENDING_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_READ_TIMEOUT: Duration = Duration::from_millis(100);
const LINK_LOG_INTERVAL: Duration = Duration::from_secs(5);

const INPUT_INTERVAL: Duration = Duration::from_millis(2);
const DSCP_EF: u32 = 0xB8;
const PAKE_ID_A: &[u8] = b"GlideLink v2 controller";
const PAKE_ID_B: &[u8] = b"GlideLink v2 agent";

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

/// Ask the OS which source address it would use for this peer. This performs
/// the route lookup without sending a packet and keeps the app out of the
/// routing-table business.
fn select_egress(peer: SocketAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect(peer).ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(IpAddr::V4(ip)),
        _ => None,
    }
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

    fn reset(&mut self) {
        self.interval = INPUT_INTERVAL;
        self.last_move_send = Instant::now()
            .checked_sub(INPUT_INTERVAL)
            .unwrap_or_else(Instant::now);
        self.rtt.reset();
    }

    fn observe_rtt(&mut self, rtt_ms: f64, status: &Status) {
        self.rtt.observe(rtt_ms);
        let jitter_ms = self.rtt.jitter_ms();
        let interval_ms = self.interval.as_secs_f64() * 1000.0;
        status.update(|s| {
            s.jitter_ms = Some(jitter_ms);
            s.input_interval_ms = Some(interval_ms);
        });
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

pub fn preview_invite(ip: Ipv4Addr) -> Result<String, String> {
    let socket = bind_free_port(ip)?;
    let port = socket
        .local_addr()
        .map_err(|e| error::detail("E_NET_BIND", e))?
        .port();
    drop(socket);
    Ok(Invite::generate(ip, port).encode())
}

pub fn start_controller(
    config: &AppConfig,
    pairing_code: &str,
    capture_rx: Receiver<CaptureMsg>,
    capture_shared: Arc<CaptureShared>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let adapt_modifiers = config.adapt_modifiers;
    let invite = Invite::decode(pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    let peer = SocketAddr::new(IpAddr::V4(invite.ip), invite.port);
    let pairing_code = invite::normalize_code(pairing_code);

    let socket = match select_egress(peer) {
        Some(ip) => match UdpSocket::bind(SocketAddr::new(ip, 0)) {
            Ok(socket) => socket,
            Err(e) => {
                log::warn!("controller bind {ip} failed: {e}; using wildcard");
                UdpSocket::bind("0.0.0.0:0").map_err(|e| error::detail("E_NET_BIND", e))?
            }
        },
        None => UdpSocket::bind("0.0.0.0:0").map_err(|e| error::detail("E_NET_BIND", e))?,
    };
    set_dscp(&socket);

    if let Ok(local) = socket.local_addr() {
        let name = match local.ip() {
            IpAddr::V4(ip) => interface_name_for_ip(ip),
            _ => None,
        };
        match name {
            Some(name) => log::info!("controller local {local} via {name} -> {peer}"),
            None => log::info!("controller local {local} -> {peer}"),
        }
    }

    let local_screen = local_screen_or_zero();
    log::info!(
        "controller target {}:{} from invite",
        invite.ip,
        invite.port
    );

    status.update(|s| {
        s.running = true;
        s.peer = Some(peer.to_string());
        s.local_screen = Some(local_screen);
        s.connected = false;
        s.remote_active = false;
        s.latency_ms = None;
        s.last_error_code = None;
        s.last_error_detail = None;
        s.remote_screen = None;
        s.started_at_ms = Some(now_ms());
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.pairing_code = None;
    });

    let reader_drops = Arc::new(AtomicU64::new(0));
    let (net_tx, net_rx) = bounded::<Datagram>(CONTROLLER_NET_QUEUE);
    let reader = spawn_reader(
        &socket,
        net_tx,
        Arc::clone(&stop),
        Arc::clone(&reader_drops),
    )?;

    let controller_stop = Arc::clone(&stop);
    let spawn = thread::Builder::new()
        .name("glidelink-controller-net".into())
        .spawn(move || {
            controller_loop(
                socket,
                peer,
                pairing_code,
                local_screen,
                adapt_modifiers,
                capture_rx,
                capture_shared,
                controller_stop,
                status,
                net_rx,
                reader,
                reader_drops,
            );
        });
    match spawn {
        Ok(handle) => Ok(handle),
        Err(e) => {
            stop.store(true, Ordering::Release);
            Err(error::detail("E_NET_THREAD", e))
        }
    }
}

struct PendingHandshake {
    sid: [u8; protocol::SID_LEN],
    state: Spake2<Ed25519Group>,
    hello_packet: Vec<u8>,
    created: Instant,
}

struct ControllerSession {
    session: Session,
    screen: ScreenInfo,
    confirmed: bool,
    last_confirm: Instant,
    confirm_deadline: Instant,
    pacer: InputPacer,
}

fn drop_controller_session(
    active: &mut Option<ControllerSession>,
    pending: &mut Option<PendingHandshake>,
    pending_move: &mut Option<RemoteInput>,
    pending_pings: &mut Vec<(u64, Instant)>,
    last_hello: &mut Instant,
    capture_shared: &CaptureShared,
    status: &Status,
    error_code: Option<&str>,
    detail: Option<String>,
) {
    active.take();
    pending.take();
    *pending_move = None;
    pending_pings.clear();
    *last_hello = Instant::now();
    capture_shared.clear_peer();
    let code = error_code.map(str::to_string);
    status.update(|s| {
        s.connected = false;
        s.remote_screen = None;
        s.latency_ms = None;
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.last_error_code = code;
        s.last_error_detail = detail;
    });
}

fn send_encrypted(
    session: &mut Session,
    kind: u8,
    payload: &[u8],
    socket: &UdpSocket,
    peer: SocketAddr,
    status: &Status,
) -> Result<(), String> {
    let packet = session.encrypt_packet(kind, payload);
    send_packet(socket, &packet, peer, status)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn controller_loop(
    socket: UdpSocket,
    peer: SocketAddr,
    pairing_code: String,
    local_screen: ScreenInfo,
    adapt_modifiers: bool,
    capture_rx: Receiver<CaptureMsg>,
    capture_shared: Arc<CaptureShared>,
    stop: Arc<AtomicBool>,
    status: Status,
    net_rx: Receiver<Datagram>,
    reader: JoinHandle<()>,
    reader_drops: Arc<AtomicU64>,
) {
    crate::sched::boost_current_thread();
    let _session_guard = crate::sched::SessionGuard::begin();
    let started = Instant::now();
    let mut pending: Option<PendingHandshake> = None;
    let mut active: Option<ControllerSession> = None;
    let mut pending_move: Option<RemoteInput> = None;
    let mut last_sent_buttons = 0u8;
    let mut last_hello = Instant::now()
        .checked_sub(CONTROLLER_HANDSHAKE_RETRY)
        .unwrap_or_else(Instant::now);
    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();
    let mut pending_pings: Vec<(u64, Instant)> = Vec::new();
    let mut next_ping_id = 1u64;
    let mut unreachable_reported = false;
    let mut pending_capture: Option<CaptureMsg> = None;
    let mut pending_net: Option<Datagram> = None;
    let mut capture_source = capture_rx.clone();
    let mut net_source = net_rx.clone();
    let tick = tick(Duration::from_millis(1));
    let mut max_lateness_ms = 0.0f64;
    let mut last_diag = Instant::now();
    let mut peer_platform = protocol::PLATFORM_UNKNOWN;
    let mut last_key_packet: Option<KeyPacket> = None;
    let mut last_key_send = Instant::now();

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();

        // Drain capture first, but keep only the latest move. Button and wheel
        // changes are sent immediately so clicks are never delayed.
        loop {
            let message = if let Some(message) = pending_capture.take() {
                message
            } else {
                match capture_rx.try_recv() {
                    Ok(message) => message,
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        capture_source = never();
                        last_key_packet = Some(KeyPacket::INACTIVE);
                        break;
                    }
                }
            };
            let input = match message {
                CaptureMsg::Mouse(input) => input,
                CaptureMsg::Keys(mut packet) => {
                    let Some(current) = active.as_mut() else {
                        continue;
                    };
                    if !current.confirmed {
                        continue;
                    }
                    if adapt_modifiers
                        && peer_platform != 0
                        && peer_platform != input::platform_id()
                    {
                        adapt_key_packet(&mut packet);
                    }
                    if let Some(queued) = pending_move.take() {
                        if let Err(e) = send_encrypted(
                            &mut current.session,
                            TYPE_INPUT,
                            &encode_input(&queued),
                            &socket,
                            peer,
                            &status,
                        ) {
                            log::warn!("input send failed: {e}");
                            drop_controller_session(
                                &mut active,
                                &mut pending,
                                &mut pending_move,
                                &mut pending_pings,
                                &mut last_hello,
                                &capture_shared,
                                &status,
                                Some("NET_SEND_FAILED"),
                                Some(e),
                            );
                            last_sent_buttons = 0;
                            break;
                        }
                    }
                    if let Err(e) = send_encrypted(
                        &mut current.session,
                        TYPE_KEY,
                        &encode_key(&packet),
                        &socket,
                        peer,
                        &status,
                    ) {
                        log::warn!("key state send failed: {e}");
                        drop_controller_session(
                            &mut active,
                            &mut pending,
                            &mut pending_move,
                            &mut pending_pings,
                            &mut last_hello,
                            &capture_shared,
                            &status,
                            Some("NET_SEND_FAILED"),
                            Some(e),
                        );
                        last_sent_buttons = 0;
                        break;
                    }
                    last_key_packet = Some(packet);
                    last_key_send = now;
                    continue;
                }
            };

            let Some(current) = active.as_mut() else {
                continue;
            };
            if !current.confirmed {
                continue;
            }
            let must_send = input.buttons != last_sent_buttons || input.wheel != 0;
            if !must_send {
                pending_move = Some(input);
                continue;
            }
            if let Some(queued) = pending_move.take() {
                if let Err(e) = send_encrypted(
                    &mut current.session,
                    TYPE_INPUT,
                    &encode_input(&queued),
                    &socket,
                    peer,
                    &status,
                ) {
                    log::warn!("input send failed: {e}");
                    drop_controller_session(
                        &mut active,
                        &mut pending,
                        &mut pending_move,
                        &mut pending_pings,
                        &mut last_hello,
                        &capture_shared,
                        &status,
                        Some("NET_SEND_FAILED"),
                        Some(e),
                    );
                    last_sent_buttons = 0;
                    break;
                }
            }
            if let Err(e) = send_encrypted(
                &mut current.session,
                TYPE_INPUT,
                &encode_input(&input),
                &socket,
                peer,
                &status,
            ) {
                log::warn!("input send failed: {e}");
                drop_controller_session(
                    &mut active,
                    &mut pending,
                    &mut pending_move,
                    &mut pending_pings,
                    &mut last_hello,
                    &capture_shared,
                    &status,
                    Some("NET_SEND_FAILED"),
                    Some(e),
                );
                last_sent_buttons = 0;
                break;
            }
            last_sent_buttons = input.buttons;
            current.pacer.last_move_send = now;
        }

        if let Some(current) = active.as_mut() {
            if current.confirmed {
                if let Some(input) = pending_move {
                    let elapsed = now.saturating_duration_since(current.pacer.last_move_send);
                    if elapsed >= current.pacer.interval {
                        let lateness_ms =
                            elapsed.saturating_sub(current.pacer.interval).as_secs_f64() * 1000.0;
                        max_lateness_ms = max_lateness_ms.max(lateness_ms);
                        if let Err(e) = send_encrypted(
                            &mut current.session,
                            TYPE_INPUT,
                            &encode_input(&input),
                            &socket,
                            peer,
                            &status,
                        ) {
                            log::warn!("input send failed: {e}");
                            pending_move = None;
                            drop_controller_session(
                                &mut active,
                                &mut pending,
                                &mut pending_move,
                                &mut pending_pings,
                                &mut last_hello,
                                &capture_shared,
                                &status,
                                Some("NET_SEND_FAILED"),
                                Some(e),
                            );
                            last_sent_buttons = 0;
                        } else {
                            pending_move = None;
                            current.pacer.last_move_send = now;
                        }
                    }
                }
            } else {
                pending_move = None;
            }
        } else {
            pending_move = None;
        }

        if active.is_none() {
            if pending
                .as_ref()
                .map(|p| p.created.elapsed() > CONTROLLER_HANDSHAKE_TIMEOUT)
                .unwrap_or(false)
            {
                pending = None;
            }
            if now.saturating_duration_since(last_hello) >= CONTROLLER_HANDSHAKE_RETRY {
                if pending.is_none() {
                    let sid = random_sid();
                    let (state, pake_msg) = Spake2::<Ed25519Group>::start_a(
                        &Password::new(pairing_code.as_bytes()),
                        &Identity::new(PAKE_ID_A),
                        &Identity::new(PAKE_ID_B),
                    );
                    debug_assert_eq!(pake_msg.len(), PAKE_MSG_LEN);
                    let hello = Handshake {
                        sid,
                        screen: local_screen,
                        timestamp_ms: now_ms(),
                        platform: input::platform_id(),
                    };
                    let hello_packet = build_pake_hello(&hello, &pake_msg);
                    pending = Some(PendingHandshake {
                        sid,
                        state,
                        hello_packet,
                        created: now,
                    });
                }
                if let Some(p) = pending.as_ref() {
                    if let Err(e) = send_packet(&socket, &p.hello_packet, peer, &status) {
                        log::warn!("handshake send failed: {e}");
                    }
                }
                last_hello = now;

                if !unreachable_reported && started.elapsed() >= CONTROLLER_UNREACHABLE_HINT_AFTER {
                    unreachable_reported = true;
                    status.set_error("NET_NO_RESPONSE", None);
                }
            }
        } else if let Some(current) = active.as_mut() {
            if !current.confirmed {
                if now.saturating_duration_since(current.last_confirm) >= CONTROLLER_CONFIRM_RETRY {
                    if let Err(e) = send_encrypted(
                        &mut current.session,
                        TYPE_CONFIRM,
                        CONFIRM_PAYLOAD,
                        &socket,
                        peer,
                        &status,
                    ) {
                        log::warn!("confirm send failed: {e}");
                        drop_controller_session(
                            &mut active,
                            &mut pending,
                            &mut pending_move,
                            &mut pending_pings,
                            &mut last_hello,
                            &capture_shared,
                            &status,
                            Some("NET_SEND_FAILED"),
                            Some(e),
                        );
                        last_sent_buttons = 0;
                        continue;
                    }
                    current.last_confirm = now;
                }
                if now >= current.confirm_deadline {
                    log::warn!("PAKE confirmation timed out");
                    drop_controller_session(
                        &mut active,
                        &mut pending,
                        &mut pending_move,
                        &mut pending_pings,
                        &mut last_hello,
                        &capture_shared,
                        &status,
                        Some("NET_TIMEOUT"),
                        None,
                    );
                    last_sent_buttons = 0;
                    continue;
                }
            } else {
                if now.saturating_duration_since(last_ping) >= CONTROLLER_PING_INTERVAL {
                    let id = next_ping_id;
                    next_ping_id = next_ping_id.wrapping_add(1).max(1);
                    if let Err(e) = send_encrypted(
                        &mut current.session,
                        TYPE_PING,
                        &encode_u64(id),
                        &socket,
                        peer,
                        &status,
                    ) {
                        log::warn!("ping send failed: {e}");
                    } else {
                        pending_pings.push((id, Instant::now()));
                        if pending_pings.len() > 32 {
                            pending_pings.remove(0);
                        }
                    }
                    last_ping = now;
                }

                if now.saturating_duration_since(last_key_send) >= CONTROLLER_KEY_INTERVAL {
                    let mut packet = last_key_packet.unwrap_or(KeyPacket::INACTIVE);
                    packet.event = KeyEvent::None;
                    if let Err(e) = send_encrypted(
                        &mut current.session,
                        TYPE_KEY,
                        &encode_key(&packet),
                        &socket,
                        peer,
                        &status,
                    ) {
                        log::warn!("key state send failed: {e}");
                    }
                    last_key_send = now;
                }
            }
        }

        loop {
            let (packet, from) = if let Some(datagram) = pending_net.take() {
                datagram
            } else {
                match net_rx.try_recv() {
                    Ok(datagram) => datagram,
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        log::warn!("controller receive thread stopped");
                        net_source = never();
                        break;
                    }
                }
            };
            status.add_received(packet.len());
            if from != peer {
                continue;
            }
            if active.is_none() {
                if packet.len() != protocol::ACCEPT_LEN {
                    continue;
                }
                let parsed = pending
                    .as_ref()
                    .and_then(|p| parse_pake_accept(&packet, &p.sid).ok());
                let Some((accept, pake_msg)) = parsed else {
                    continue;
                };
                let Some(p) = pending.take() else {
                    continue;
                };
                let shared = match p.state.finish(&pake_msg) {
                    Ok(key) => key,
                    Err(_) => {
                        log::debug!("PAKE exchange failed");
                        last_hello = Instant::now();
                        continue;
                    }
                };
                let transcript = transcript_hash(&p.hello_packet, &packet);
                let (c2a, a2c) = derive_session_keys(&shared, &p.sid, &transcript);
                let mut session = Session::from_keys(&c2a, &a2c, p.sid, Side::Controller);
                if let Err(e) = send_encrypted(
                    &mut session,
                    TYPE_CONFIRM,
                    CONFIRM_PAYLOAD,
                    &socket,
                    peer,
                    &status,
                ) {
                    log::warn!("confirm send failed: {e}");
                    last_hello = Instant::now();
                    continue;
                }
                peer_platform = accept.platform;
                active = Some(ControllerSession {
                    session,
                    screen: accept.screen,
                    confirmed: false,
                    last_confirm: now,
                    confirm_deadline: now + CONTROLLER_CONFIRM_TIMEOUT,
                    pacer: InputPacer::new(),
                });
                unreachable_reported = false;
                last_pong = now;
                last_ping = now;
                pending_pings.clear();
            } else if let Some(current) = active.as_mut() {
                match current.session.decrypt_packet(&packet) {
                    Ok((TYPE_CONFIRM_ACK, payload)) => {
                        if payload == CONFIRM_PAYLOAD {
                            if !current.confirmed {
                                current.confirmed = true;
                                current.pacer.reset();
                                let screen = current.screen;
                                capture_shared.set_peer(screen.width, screen.height);
                                status.update(|s| {
                                    s.connected = true;
                                    s.remote_screen = Some(screen);
                                    s.latency_ms = None;
                                    s.last_error_code = None;
                                    s.last_error_detail = None;
                                    s.jitter_ms = Some(0.0);
                                    s.input_interval_ms =
                                        Some(current.pacer.interval.as_secs_f64() * 1000.0);
                                    s.packet_loss_percent = None;
                                });
                                last_pong = now;
                                last_ping = now;
                                last_sent_buttons = 0;
                                pending_move = None;
                                pending_pings.clear();
                                last_key_packet = Some(KeyPacket::INACTIVE);
                                last_key_send = now;
                                log::info!(
                                    "v2 session confirmed with {peer} ({}x{})",
                                    screen.width,
                                    screen.height
                                );
                            } else {
                                last_pong = now;
                            }
                        }
                    }
                    Ok((TYPE_PONG, payload)) => {
                        if current.confirmed {
                            if let Ok((id, stats)) = decode_pong(&payload) {
                                if let Some(index) = pending_pings
                                    .iter()
                                    .position(|(pending_id, _)| *pending_id == id)
                                {
                                    let (_, sent_at) = pending_pings.remove(index);
                                    let rtt = sent_at.elapsed().as_secs_f64() * 1000.0;
                                    status.update(|s| s.latency_ms = Some(rtt));
                                    current.pacer.observe_rtt(rtt, &status);
                                }
                                if let Some((expected, received)) = stats {
                                    let loss = if expected == 0 {
                                        None
                                    } else {
                                        Some(
                                            ((expected.saturating_sub(received) as f64
                                                / expected as f64)
                                                * 100.0)
                                                .clamp(0.0, 100.0),
                                        )
                                    };
                                    status.update(|s| s.packet_loss_percent = loss);
                                }
                            }
                            pending_pings.retain(|(_, sent_at)| {
                                sent_at.elapsed() <= CONTROLLER_PONG_TIMEOUT
                            });
                            last_pong = now;
                        }
                    }
                    Ok((TYPE_BYE, _)) => {
                        drop_controller_session(
                            &mut active,
                            &mut pending,
                            &mut pending_move,
                            &mut pending_pings,
                            &mut last_hello,
                            &capture_shared,
                            &status,
                            None,
                            None,
                        );
                        last_sent_buttons = 0;
                    }
                    Ok(_) => {
                        if current.confirmed {
                            last_pong = now;
                        }
                    }
                    Err(ProtocolError::Replay) => {}
                    Err(e) => {
                        log::debug!("dropped packet: {e}");
                    }
                }
            }
        }

        let now = Instant::now();
        let timed_out = active
            .as_ref()
            .map(|current| {
                current.confirmed
                    && now.saturating_duration_since(last_pong) >= CONTROLLER_PONG_TIMEOUT
            })
            .unwrap_or(false);
        if timed_out {
            log::warn!("peer timed out");
            drop_controller_session(
                &mut active,
                &mut pending,
                &mut pending_move,
                &mut pending_pings,
                &mut last_hello,
                &capture_shared,
                &status,
                Some("NET_TIMEOUT"),
                None,
            );
            unreachable_reported = false;
            last_sent_buttons = 0;
            continue;
        }

        if now.saturating_duration_since(last_diag) >= LINK_LOG_INTERVAL {
            let (srtt, jitter, min_rtt) = active
                .as_ref()
                .map(|current| {
                    (
                        current.pacer.rtt.srtt_ms(),
                        current.pacer.rtt.jitter_ms(),
                        current.pacer.rtt.min_rtt_ms(),
                    )
                })
                .unwrap_or((None, 0.0, None));
            let loss = status.snapshot().packet_loss_percent;
            let drops = reader_drops.swap(0, Ordering::Relaxed);
            log::debug!(
                "link srtt={srtt:?} rttvar={jitter:.1} min_rtt={min_rtt:?} loss={loss:?} lateness={max_lateness_ms:.1}ms rx_drops={drops}"
            );
            max_lateness_ms = 0.0;
            last_diag = now;
        }

        crossbeam_channel::select! {
            recv(capture_source) -> message => {
                match message {
                    Ok(message) => pending_capture = Some(message),
                    Err(_) => {
                        capture_source = never();
                    }
                }
            }
            recv(net_source) -> message => {
                match message {
                    Ok(datagram) => pending_net = Some(datagram),
                    Err(_) => {
                        net_source = never();
                    }
                }
            }
            recv(tick) -> _ => {}
        }
    }

    if let Some(current) = active.as_mut() {
        if current.confirmed {
            let _ = send_encrypted(&mut current.session, TYPE_BYE, &[], &socket, peer, &status);
        }
    }
    if reader.join().is_err() {
        log::error!("controller receive thread panicked");
    }
    capture_shared.clear_peer();
}

pub fn start_agent(
    config: &AppConfig,
    pairing_code: &str,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let invite = Invite::decode(pairing_code).map_err(|_| error::code("E_PAIRING_CODE"))?;
    if config.local_ipv4()? != invite.ip {
        return Err(error::code("E_LOCAL_IP"));
    }
    let pairing_code = invite::normalize_code(pairing_code);
    let bind_addr = SocketAddr::new(IpAddr::V4(invite.ip), invite.port);
    let socket = UdpSocket::bind(bind_addr)
        .map_err(|e| error::detail("E_NET_BIND", format!("{bind_addr}: {e}")))?;
    socket
        .set_read_timeout(Some(AGENT_READ_TIMEOUT))
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;
    set_dscp(&socket);

    let local_screen = local_screen_or_zero();
    log::info!("agent listening on {bind_addr}");

    status.update(|s| {
        s.running = true;
        s.connected = false;
        s.remote_active = false;
        s.latency_ms = None;
        s.local_screen = Some(local_screen);
        s.last_error_code = None;
        s.last_error_detail = None;
        s.peer = None;
        s.remote_screen = None;
        s.started_at_ms = Some(now_ms());
        s.jitter_ms = None;
        s.input_interval_ms = None;
        s.packet_loss_percent = None;
        s.pairing_code = Some(pairing_code.clone());
    });

    thread::Builder::new()
        .name("glidelink-agent-net".into())
        .spawn(move || {
            agent_loop(socket, local_screen, pairing_code, inject_tx, stop, status);
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

struct AgentSession {
    session: Session,
    controller: SocketAddr,
    last_packet: Instant,
    loss: LossTracker,
    last_key_seq: u64,
}

struct PendingAgent {
    session: Session,
    sid: [u8; protocol::SID_LEN],
    controller: SocketAddr,
    controller_screen: ScreenInfo,
    accept_packet: Vec<u8>,
    created: Instant,
}

fn agent_loop(
    socket: UdpSocket,
    local_screen: ScreenInfo,
    pairing_code: String,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) {
    crate::sched::boost_current_thread();
    let _session_guard = crate::sched::SessionGuard::begin();
    let mut session: Option<AgentSession> = None;
    let mut pending: Option<PendingAgent> = None;
    let mut last_input = Instant::now();
    let mut active_reported = false;
    let mut pake_limiter = PakeLimiter::new();
    let mut buf = [0u8; 2048];

    while !stop.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                status.add_received(n);
                let packet = &buf[..n];
                let is_hello = packet.len() == HELLO_LEN && packet[5] == TYPE_HELLO;

                if is_hello {
                    let Ok(sid) = hello_sid(packet) else {
                        continue;
                    };

                    if let Some(p) = pending.as_ref() {
                        if p.sid == sid && p.controller == from {
                            let _ = send_packet(&socket, &p.accept_packet, from, &status);
                            continue;
                        }
                    }
                    // An active session can only be replaced after its own
                    // timeout; an unauthenticated HELLO must not tear it down.
                    if session.is_some() {
                        log::debug!("ignoring HELLO from {from} while a session is active");
                        continue;
                    }
                    if let Some(p) = pending.as_ref() {
                        if p.controller != from {
                            log::debug!(
                                "ignoring HELLO from {from} while pairing with {}",
                                p.controller
                            );
                            continue;
                        }
                    }

                    let Ok((hello, pake_msg)) = parse_pake_hello(packet) else {
                        log::debug!("rejected HELLO from {from}");
                        continue;
                    };
                    if !pake_limiter.allow() {
                        log::debug!("PAKE handshake rate limited for {from}");
                        continue;
                    }
                    let (state, accept_msg) = Spake2::<Ed25519Group>::start_b(
                        &Password::new(pairing_code.as_bytes()),
                        &Identity::new(PAKE_ID_A),
                        &Identity::new(PAKE_ID_B),
                    );
                    let shared = match state.finish(&pake_msg) {
                        Ok(key) => key,
                        Err(_) => {
                            log::debug!("PAKE exchange failed for {from}");
                            continue;
                        }
                    };
                    let accept = Handshake {
                        sid,
                        screen: local_screen,
                        timestamp_ms: now_ms(),
                        platform: input::platform_id(),
                    };
                    let accept_packet = build_pake_accept(&accept, &accept_msg);
                    let transcript = transcript_hash(packet, &accept_packet);
                    let (c2a, a2c) = derive_session_keys(&shared, &sid, &transcript);
                    let new_session = Session::from_keys(&c2a, &a2c, sid, Side::Agent);
                    if let Err(e) = send_packet(&socket, &accept_packet, from, &status) {
                        log::warn!("accept send failed: {e}");
                        continue;
                    }
                    pending = Some(PendingAgent {
                        session: new_session,
                        sid,
                        controller: from,
                        controller_screen: hello.screen,
                        accept_packet,
                        created: Instant::now(),
                    });
                    continue;
                }

                if session.is_none() {
                    let mut confirm_seq = None;
                    if let Some(p) = pending.as_mut() {
                        if p.controller != from {
                            continue;
                        }
                        match p.session.decrypt_packet_with_seq(packet) {
                            Ok((TYPE_CONFIRM, payload, seq)) => {
                                if payload == CONFIRM_PAYLOAD {
                                    let ack =
                                        p.session.encrypt_packet(TYPE_CONFIRM_ACK, CONFIRM_PAYLOAD);
                                    let _ = send_packet(&socket, &ack, from, &status);
                                    confirm_seq = Some(seq);
                                }
                            }
                            Ok(_) => {}
                            Err(ProtocolError::Replay) => {}
                            Err(e) => log::debug!("rejected confirm from {from}: {e}"),
                        }
                    }
                    if let Some(base_seq) = confirm_seq {
                        let Some(p) = pending.take() else {
                            continue;
                        };
                        let now = Instant::now();
                        let screen = p.controller_screen;
                        let _ = inject_tx.send(InjectMsg::Reset);
                        session = Some(AgentSession {
                            session: p.session,
                            controller: from,
                            last_packet: now,
                            loss: LossTracker::new(base_seq),
                            last_key_seq: 0,
                        });
                        last_input = now;
                        active_reported = false;
                        status.update(|s| {
                            s.connected = true;
                            s.remote_active = false;
                            s.peer = Some(from.to_string());
                            s.remote_screen = Some(screen);
                            s.last_error_code = None;
                            s.last_error_detail = None;
                            s.packet_loss_percent = None;
                        });
                        log::info!("v2 session confirmed with {from}");
                    }
                    continue;
                }

                let Some(active) = session.as_mut() else {
                    continue;
                };
                if active.controller != from {
                    continue;
                }

                let mut close_session = false;
                match active.session.decrypt_packet_with_seq(packet) {
                    Ok((kind, payload, seq)) => {
                        active.loss.observe(seq);
                        match kind {
                            TYPE_INPUT => {
                                if let Ok(input) = decode_input(&payload) {
                                    let _ = inject_tx.send(InjectMsg::State(input));
                                    let now = Instant::now();
                                    active.last_packet = now;
                                    last_input = now;
                                    if !active_reported {
                                        active_reported = true;
                                        status.update(|s| {
                                            s.remote_active = true;
                                            s.last_error_code = None;
                                            s.last_error_detail = None;
                                        });
                                    }
                                }
                            }
                            TYPE_PING => {
                                active.last_packet = Instant::now();
                                if let Ok(id) = decode_u64(&payload) {
                                    let expected = active.loss.expected();
                                    let received = active.loss.received();
                                    let response = active.session.encrypt_packet(
                                        TYPE_PONG,
                                        &encode_pong(id, expected, received),
                                    );
                                    let _ = send_packet(&socket, &response, from, &status);
                                    let loss = active.loss.loss_percent();
                                    status.update(|s| s.packet_loss_percent = loss);
                                }
                            }
                            TYPE_KEY => {
                                let now = Instant::now();
                                active.last_packet = now;
                                if seq > active.last_key_seq {
                                    if let Ok(packet) = decode_key(&payload) {
                                        active.last_key_seq = seq;
                                        let _ = inject_tx.send(InjectMsg::Keys(packet));
                                        if packet.active {
                                            last_input = now;
                                            if !active_reported {
                                                active_reported = true;
                                                status.update(|s| {
                                                    s.remote_active = true;
                                                    s.last_error_code = None;
                                                    s.last_error_detail = None;
                                                });
                                            }
                                        } else {
                                            active_reported = false;
                                            status.update(|s| s.remote_active = false);
                                        }
                                    } else {
                                        log::debug!("rejected key state from {from}");
                                    }
                                }
                            }
                            TYPE_CONFIRM => {
                                active.last_packet = Instant::now();
                                if payload == CONFIRM_PAYLOAD {
                                    let ack = active
                                        .session
                                        .encrypt_packet(TYPE_CONFIRM_ACK, CONFIRM_PAYLOAD);
                                    let _ = send_packet(&socket, &ack, from, &status);
                                }
                            }
                            TYPE_BYE => {
                                close_session = true;
                            }
                            _ => {}
                        }
                    }
                    Err(ProtocolError::Replay) => {}
                    Err(e) => {
                        log::debug!("rejected encrypted packet from {from}: {e}");
                    }
                }

                if close_session {
                    let _ = inject_tx.send(InjectMsg::Reset);
                    session = None;
                    active_reported = false;
                    status.update(|s| {
                        s.connected = false;
                        s.remote_active = false;
                        s.peer = None;
                        s.remote_screen = None;
                        s.latency_ms = None;
                        s.packet_loss_percent = None;
                    });
                }
            }
            Err(e) if is_benign_recv_error(&e) => {}
            Err(e) => {
                log::warn!("UDP receive error: {e}");
                thread::sleep(Duration::from_millis(10));
            }
        }

        let now = Instant::now();

        if let Some(p) = pending.as_ref() {
            if now.saturating_duration_since(p.created) >= AGENT_PENDING_TIMEOUT {
                pending = None;
            }
        }

        if active_reported && now.saturating_duration_since(last_input) >= AGENT_ACTIVE_TIMEOUT {
            active_reported = false;
            status.update(|s| s.remote_active = false);
        }

        let timed_out = session
            .as_ref()
            .map(|active| {
                now.saturating_duration_since(active.last_packet) >= AGENT_SESSION_TIMEOUT
            })
            .unwrap_or(false);
        if timed_out {
            log::warn!("controller timed out");
            let _ = inject_tx.send(InjectMsg::Reset);
            session = None;
            active_reported = false;
            status.update(|s| {
                s.connected = false;
                s.remote_active = false;
                s.peer = None;
                s.remote_screen = None;
                s.latency_ms = None;
                s.packet_loss_percent = None;
                s.last_error_code = Some("NET_PEER_DISCONNECTED".into());
                s.last_error_detail = None;
            });
        }
    }

    let _ = inject_tx.send(InjectMsg::Reset);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Role;
    use crate::input::BUTTON_LEFT;
    use crate::layout::Edge;
    use crossbeam_channel::unbounded;
    use std::net::Ipv4Addr;

    #[test]
    fn constants_fit_in_udp_buffers() {
        assert!(protocol::HEADER_LEN + protocol::INPUT_LEN + protocol::TAG_LEN < 2048);
        assert!(protocol::HEADER_LEN + protocol::KEY_LEN + protocol::TAG_LEN < 2048);
        assert_eq!(protocol::HELLO_LEN, 72);
    }

    #[test]
    fn loopback_handshake_and_input_forwarding() {
        let agent_config = AppConfig {
            role: Role::Agent,
            local_ip: "127.0.0.1".into(),
            edge: Edge::Right,
            ..AppConfig::default()
        };
        let controller_config = AppConfig {
            role: Role::Controller,
            edge: Edge::Right,
            ..AppConfig::default()
        };

        let agent_status = Status::new(None);
        let controller_status = Status::new(None);
        let (inject_tx, inject_rx) = unbounded::<InjectMsg>();
        let (capture_tx, capture_rx) = unbounded::<CaptureMsg>();
        let agent_stop = Arc::new(AtomicBool::new(false));
        let controller_stop = Arc::new(AtomicBool::new(false));
        let capture_shared = CaptureShared::new(controller_status.clone());

        let pairing_code = preview_invite(Ipv4Addr::LOCALHOST).expect("preview invite");
        let invite = Invite::decode(&pairing_code).expect("decode invite");
        assert_eq!(invite.ip, Ipv4Addr::LOCALHOST);
        assert!((PORT_RANGE_START..=PORT_RANGE_END).contains(&invite.port));

        let agent_handle = start_agent(
            &agent_config,
            &pairing_code,
            inject_tx,
            Arc::clone(&agent_stop),
            agent_status.clone(),
        )
        .expect("start agent");
        assert_eq!(
            agent_status.snapshot().pairing_code.as_deref(),
            Some(pairing_code.as_str())
        );

        let controller_handle = start_controller(
            &controller_config,
            &pairing_code,
            capture_rx,
            capture_shared,
            Arc::clone(&controller_stop),
            controller_status.clone(),
        )
        .expect("start controller");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !controller_status.snapshot().connected && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            controller_status.snapshot().connected,
            "controller did not connect within the deadline"
        );

        // The 100ms heartbeat must produce RTT and receiver-side loss stats on
        // both ends on a clean loopback link.
        let stats_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let controller = controller_status.snapshot();
            let agent = agent_status.snapshot();
            if controller.latency_ms.is_some()
                && controller.packet_loss_percent == Some(0.0)
                && agent.packet_loss_percent == Some(0.0)
            {
                break;
            }
            if Instant::now() >= stats_deadline {
                panic!("link stats did not arrive: controller={controller:?} agent={agent:?}");
            }
            thread::sleep(Duration::from_millis(20));
        }

        capture_tx
            .send(CaptureMsg::Mouse(RemoteInput {
                x: 0.25,
                y: 0.75,
                buttons: BUTTON_LEFT,
                wheel: -1,
            }))
            .expect("send input");

        let received = loop {
            match inject_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(InjectMsg::State(input)) => break input,
                Ok(InjectMsg::Keys(_)) => continue,
                Ok(InjectMsg::Reset) => continue,
                Err(e) => panic!("agent did not forward input: {e}"),
            }
        };
        assert!((received.x - 0.25).abs() < 0.001);
        assert!((received.y - 0.75).abs() < 0.001);
        assert_eq!(received.buttons, BUTTON_LEFT);
        assert_eq!(received.wheel, -1);
        assert!(agent_status.snapshot().connected);

        // A move without a button/wheel change goes through the paced
        // coalescing path instead of being sent immediately.
        capture_tx
            .send(CaptureMsg::Mouse(RemoteInput {
                x: 0.5,
                y: 0.5,
                buttons: 0,
                wheel: 0,
            }))
            .expect("send move");
        let moved = loop {
            match inject_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(InjectMsg::State(input)) => break input,
                Ok(InjectMsg::Keys(_)) => continue,
                Ok(InjectMsg::Reset) => continue,
                Err(e) => panic!("agent did not forward move: {e}"),
            }
        };
        assert!((moved.x - 0.5).abs() < 0.001);
        assert!((moved.y - 0.5).abs() < 0.001);
        assert!(controller_status.snapshot().packets_sent > 0);
        assert!(controller_status.snapshot().bytes_sent > 0);

        let mut keys = crate::input::keymap::KeyState::default();
        keys.insert(0x04); // A
        capture_tx
            .send(CaptureMsg::Keys(KeyPacket {
                active: true,
                caps_lock: false,
                state: keys,
                event: KeyEvent::None,
            }))
            .expect("send key state");
        let forwarded_key = loop {
            match inject_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(InjectMsg::Keys(packet)) if packet.state.contains(0x04) => break packet,
                Ok(_) => continue,
                Err(e) => panic!("agent did not forward key state: {e}"),
            }
        };
        assert!(forwarded_key.active);
        assert!(agent_status.snapshot().remote_active);

        capture_tx
            .send(CaptureMsg::Keys(KeyPacket {
                active: false,
                caps_lock: false,
                state: crate::input::keymap::KeyState::default(),
                event: KeyEvent::None,
            }))
            .expect("send key release");
        loop {
            match inject_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(InjectMsg::Keys(packet)) if !packet.active => {
                    assert!(packet.state.is_empty());
                    break;
                }
                Ok(_) => continue,
                Err(e) => panic!("agent did not receive inactive key state: {e}"),
            }
        }

        controller_stop.store(true, Ordering::Release);
        agent_stop.store(true, Ordering::Release);
        drop(capture_tx);
        drop(inject_rx);
        controller_handle.join().expect("join controller");
        agent_handle.join().expect("join agent");
    }
}
