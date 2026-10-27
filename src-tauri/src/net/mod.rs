pub mod protocol;

use crate::config::AppConfig;
use crate::error;
use crate::input::{self, CaptureShared, InjectMsg, RemoteInput};
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::{Receiver, Sender};
use protocol::{
    build_accept, build_hello, decode_input, decode_u64, derive_master, encode_input, encode_u64,
    hello_sid, now_ms, parse_accept, parse_hello, random_sid, Handshake, ProtocolError, Session,
    Side, TYPE_ACCEPT, TYPE_BYE, TYPE_HELLO, TYPE_INPUT, TYPE_PING, TYPE_PONG,
};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CONTROLLER_HANDSHAKE_RETRY: Duration = Duration::from_millis(500);
const CONTROLLER_PING_INTERVAL: Duration = Duration::from_millis(250);
const CONTROLLER_PONG_TIMEOUT: Duration = Duration::from_millis(2_500);
const CONTROLLER_UNREACHABLE_HINT_AFTER: Duration = Duration::from_secs(8);
const AGENT_SESSION_TIMEOUT: Duration = Duration::from_millis(3_000);
const AGENT_ACTIVE_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROLLER_READ_TIMEOUT: Duration = Duration::from_millis(2);
const AGENT_READ_TIMEOUT: Duration = Duration::from_millis(100);

fn local_screen_or_zero() -> ScreenInfo {
    input::screen_info().unwrap_or(ScreenInfo {
        width: 0,
        height: 0,
    })
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

pub fn start_controller(
    config: &AppConfig,
    capture_rx: Receiver<RemoteInput>,
    capture_shared: Arc<CaptureShared>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let peer = config.peer_socket_addr()?;
    if peer.is_ipv6() {
        return Err(error::code("E_IPV6_UNSUPPORTED"));
    }

    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|e| error::detail("E_NET_BIND", e))?;
    socket
        .set_read_timeout(Some(CONTROLLER_READ_TIMEOUT))
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;

    let local_screen = local_screen_or_zero();
    let passcode = config.passcode.clone();

    status.update(|s| {
        s.running = true;
        s.peer = Some(peer.to_string());
        s.local_screen = Some(local_screen);
        s.connected = false;
        s.remote_active = false;
        s.latency_ms = None;
        s.last_error_code = None;
        s.last_error_detail = None;
    });

    thread::Builder::new()
        .name("glidelink-controller-net".into())
        .spawn(move || {
            controller_loop(
                socket,
                peer,
                passcode,
                local_screen,
                capture_rx,
                capture_shared,
                stop,
                status,
            );
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

struct PendingHandshake {
    sid: [u8; protocol::SID_LEN],
    master: [u8; 32],
    packet: Vec<u8>,
}

fn controller_loop(
    socket: UdpSocket,
    peer: SocketAddr,
    passcode: String,
    local_screen: ScreenInfo,
    capture_rx: Receiver<RemoteInput>,
    capture_shared: Arc<CaptureShared>,
    stop: Arc<AtomicBool>,
    status: Status,
) {
    let started = Instant::now();
    let mut pending: Option<PendingHandshake> = None;
    let mut session: Option<Session> = None;
    let mut last_hello = Instant::now()
        .checked_sub(CONTROLLER_HANDSHAKE_RETRY)
        .unwrap_or_else(Instant::now);
    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();
    let mut pending_ping: Option<(u64, Instant)> = None;
    let mut next_ping_id = 1u64;
    let mut unreachable_reported = false;
    let mut buf = [0u8; 2048];

    while !stop.load(Ordering::Acquire) {
        // Drain input first so a burst is not delayed by socket polling.
        loop {
            match capture_rx.try_recv() {
                Ok(input) => {
                    let packet = session
                        .as_mut()
                        .map(|active| active.encrypt_packet(TYPE_INPUT, &encode_input(&input)));
                    let Some(packet) = packet else {
                        continue;
                    };
                    if let Err(e) = socket.send_to(&packet, peer) {
                        log::warn!("input send failed: {e}");
                        session = None;
                        capture_shared.clear_peer();
                        status.update(|s| {
                            s.connected = false;
                            s.remote_screen = None;
                            s.latency_ms = None;
                            s.last_error_code = Some("NET_SEND_FAILED".into());
                            s.last_error_detail = Some(e.to_string());
                        });
                        pending = None;
                        pending_ping = None;
                        last_hello = Instant::now();
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }

        let now = Instant::now();

        if session.is_none() {
            if now.saturating_duration_since(last_hello) >= CONTROLLER_HANDSHAKE_RETRY {
                if pending.is_none() {
                    let sid = random_sid();
                    let master = derive_master(&passcode, &sid);
                    let hello = Handshake {
                        sid,
                        screen: local_screen,
                        timestamp_ms: now_ms(),
                    };
                    let packet = build_hello(&master, &hello);
                    pending = Some(PendingHandshake {
                        sid,
                        master,
                        packet,
                    });
                }
                if let Some(p) = pending.as_ref() {
                    if let Err(e) = socket.send_to(&p.packet, peer) {
                        log::warn!("handshake send failed: {e}");
                    }
                }
                last_hello = now;

                if !unreachable_reported && started.elapsed() >= CONTROLLER_UNREACHABLE_HINT_AFTER {
                    unreachable_reported = true;
                    status.set_error("NET_NO_RESPONSE", None);
                }
            }
        } else {
            if now.saturating_duration_since(last_ping) >= CONTROLLER_PING_INTERVAL {
                if let Some(active) = session.as_mut() {
                    let id = next_ping_id;
                    next_ping_id = next_ping_id.wrapping_add(1).max(1);
                    let packet = active.encrypt_packet(TYPE_PING, &encode_u64(id));
                    if let Err(e) = socket.send_to(&packet, peer) {
                        log::warn!("ping send failed: {e}");
                    } else {
                        pending_ping = Some((id, Instant::now()));
                    }
                }
                last_ping = now;
            }

            if now.saturating_duration_since(last_pong) >= CONTROLLER_PONG_TIMEOUT {
                log::warn!("peer timed out");
                session = None;
                pending = None;
                pending_ping = None;
                last_hello = now;
                unreachable_reported = false;
                capture_shared.clear_peer();
                status.update(|s| {
                    s.connected = false;
                    s.remote_screen = None;
                    s.latency_ms = None;
                    s.last_error_code = Some("NET_TIMEOUT".into());
                    s.last_error_detail = None;
                });
                continue;
            }
        }

        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if from != peer {
                    continue;
                }
                if session.is_none() {
                    if n == protocol::ACCEPT_LEN {
                        if let Some(p) = pending.as_ref() {
                            if let Ok(accept) = parse_accept(&buf[..n], &p.master, &p.sid) {
                                let master = p.master;
                                let sid = p.sid;
                                let agent_screen = accept.screen;
                                session =
                                    Some(Session::from_master(&master, sid, Side::Controller));
                                pending = None;
                                pending_ping = None;
                                unreachable_reported = false;
                                last_pong = Instant::now();
                                last_ping = Instant::now();
                                capture_shared.set_peer(agent_screen.width, agent_screen.height);
                                status.update(|s| {
                                    s.connected = true;
                                    s.remote_screen = Some(agent_screen);
                                    s.last_error_code = None;
                                    s.last_error_detail = None;
                                });
                            }
                        }
                    }
                } else if let Some(active) = session.as_mut() {
                    match active.decrypt_packet(&buf[..n]) {
                        Ok((TYPE_PONG, payload)) => {
                            if let Ok(id) = decode_u64(&payload) {
                                if let Some((pending_id, sent_at)) = pending_ping {
                                    if pending_id == id {
                                        let rtt = sent_at.elapsed().as_secs_f64() * 1000.0;
                                        status.update(|s| s.latency_ms = Some(rtt));
                                        pending_ping = None;
                                    }
                                }
                            }
                            last_pong = Instant::now();
                        }
                        Ok((TYPE_BYE, _)) => {
                            session = None;
                            pending = None;
                            capture_shared.clear_peer();
                            status.update(|s| {
                                s.connected = false;
                                s.remote_screen = None;
                                s.latency_ms = None;
                            });
                            last_hello = Instant::now();
                        }
                        Ok(_) => {
                            last_pong = Instant::now();
                        }
                        Err(ProtocolError::Replay) => {}
                        Err(e) => {
                            log::debug!("dropped packet: {e}");
                        }
                    }
                }
            }
            Err(e) if is_benign_recv_error(&e) => {}
            Err(e) => {
                log::warn!("UDP receive error: {e}");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    if let Some(active) = session.as_mut() {
        let bye = active.encrypt_packet(TYPE_BYE, &[]);
        let _ = socket.send_to(&bye, peer);
    }
    capture_shared.clear_peer();
}

pub fn start_agent(
    config: &AppConfig,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let bind_addr = format!("0.0.0.0:{}", config.listen_port);
    let socket = UdpSocket::bind(&bind_addr)
        .map_err(|e| error::detail("E_NET_BIND", format!("{bind_addr}: {e}")))?;
    socket
        .set_read_timeout(Some(AGENT_READ_TIMEOUT))
        .map_err(|e| error::detail("E_NET_SOCKET", e))?;

    let local_screen = local_screen_or_zero();
    let passcode = config.passcode.clone();

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
    });

    thread::Builder::new()
        .name("glidelink-agent-net".into())
        .spawn(move || {
            agent_loop(socket, local_screen, passcode, inject_tx, stop, status);
        })
        .map_err(|e| error::detail("E_NET_THREAD", e))
}

struct AgentSession {
    session: Session,
    sid: [u8; protocol::SID_LEN],
    master: [u8; 32],
    controller: SocketAddr,
    last_packet: Instant,
}

fn agent_loop(
    socket: UdpSocket,
    local_screen: ScreenInfo,
    passcode: String,
    inject_tx: Sender<InjectMsg>,
    stop: Arc<AtomicBool>,
    status: Status,
) {
    let mut session: Option<AgentSession> = None;
    let mut last_input = Instant::now();
    let mut active_reported = false;
    let mut buf = [0u8; 2048];

    while !stop.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let packet = &buf[..n];
                let is_hello = packet.len() == protocol::HELLO_LEN && packet[5] == TYPE_HELLO;

                if is_hello {
                    let Ok(sid) = hello_sid(packet) else {
                        continue;
                    };

                    // Retransmitted HELLO for the current session: resend the
                    // accept without resetting the receive window.
                    if let Some(current) = session.as_ref() {
                        if current.sid == sid && current.controller == from {
                            let accept = Handshake {
                                sid,
                                screen: local_screen,
                                timestamp_ms: now_ms(),
                            };
                            let response = build_accept(&current.master, &accept);
                            let _ = socket.send_to(&response, from);
                            continue;
                        }
                    }

                    let master = derive_master(&passcode, &sid);
                    let Ok(hello) = parse_hello(packet, &master) else {
                        log::debug!("rejected HELLO from {from}");
                        continue;
                    };

                    let response = build_accept(
                        &master,
                        &Handshake {
                            sid,
                            screen: local_screen,
                            timestamp_ms: now_ms(),
                        },
                    );
                    if let Err(e) = socket.send_to(&response, from) {
                        log::warn!("accept send failed: {e}");
                        continue;
                    }

                    let _ = inject_tx.send(InjectMsg::Reset);
                    let now = Instant::now();
                    session = Some(AgentSession {
                        session: Session::from_master(&master, sid, Side::Agent),
                        sid,
                        master,
                        controller: from,
                        last_packet: now,
                    });
                    last_input = now;
                    active_reported = false;
                    status.update(|s| {
                        s.connected = true;
                        s.remote_active = false;
                        s.peer = Some(from.to_string());
                        s.remote_screen = Some(hello.screen);
                        s.last_error_code = None;
                        s.last_error_detail = None;
                    });
                    continue;
                }

                let Some(active) = session.as_mut() else {
                    continue;
                };
                if active.controller != from {
                    continue;
                }

                let mut close_session = false;
                match active.session.decrypt_packet(packet) {
                    Ok((TYPE_INPUT, payload)) => {
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
                    Ok((TYPE_PING, payload)) => {
                        active.last_packet = Instant::now();
                        if payload.len() == 8 {
                            let response = active.session.encrypt_packet(TYPE_PONG, &payload);
                            let _ = socket.send_to(&response, from);
                        }
                    }
                    Ok((TYPE_BYE, _)) => {
                        close_session = true;
                    }
                    Ok((TYPE_ACCEPT, _)) | Ok((TYPE_HELLO, _)) => {}
                    Ok(_) => {}
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

    #[test]
    fn constants_fit_in_udp_buffers() {
        assert!(
            protocol::HEADER_LEN + protocol::INPUT_LEN + protocol::TAG_LEN < 2048
        );
        assert_eq!(protocol::HELLO_LEN, 62);
    }

    #[test]
    fn pong_payload_is_echoed_verbatim() {
        let value = 42u64;
        let payload = encode_u64(value);
        assert_eq!(decode_u64(&payload).unwrap(), value);
    }

    #[test]
    fn loopback_handshake_and_input_forwarding() {
        let port = {
            let socket = UdpSocket::bind("127.0.0.1:0").expect("ephemeral port");
            socket.local_addr().expect("local addr").port()
        };
        let passcode = "loopback-test-code".to_string();
        let agent_config = AppConfig {
            role: Role::Agent,
            peer_addr: String::new(),
            listen_port: port,
            passcode: passcode.clone(),
            edge: Edge::Right,
        };
        let controller_config = AppConfig {
            role: Role::Controller,
            peer_addr: format!("127.0.0.1:{port}"),
            listen_port: 0,
            passcode,
            edge: Edge::Right,
        };

        let agent_status = Status::new(None);
        let controller_status = Status::new(None);
        let (inject_tx, inject_rx) = unbounded::<InjectMsg>();
        let (capture_tx, capture_rx) = unbounded::<RemoteInput>();
        let agent_stop = Arc::new(AtomicBool::new(false));
        let controller_stop = Arc::new(AtomicBool::new(false));
        let capture_shared = CaptureShared::new(controller_status.clone());

        let agent_handle = start_agent(
            &agent_config,
            inject_tx,
            Arc::clone(&agent_stop),
            agent_status.clone(),
        )
        .expect("start agent");
        let controller_handle = start_controller(
            &controller_config,
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

        capture_tx
            .send(RemoteInput {
                x: 0.25,
                y: 0.75,
                buttons: BUTTON_LEFT,
                wheel: -1,
            })
            .expect("send input");

        let received = loop {
            match inject_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(InjectMsg::State(input)) => break input,
                Ok(InjectMsg::Reset) => continue,
                Err(e) => panic!("agent did not forward input: {e}"),
            }
        };
        assert!((received.x - 0.25).abs() < 0.001);
        assert!((received.y - 0.75).abs() < 0.001);
        assert_eq!(received.buttons, BUTTON_LEFT);
        assert_eq!(received.wheel, -1);
        assert!(agent_status.snapshot().connected);

        controller_stop.store(true, Ordering::Release);
        agent_stop.store(true, Ordering::Release);
        drop(capture_tx);
        drop(inject_rx);
        controller_handle.join().expect("join controller");
        agent_handle.join().expect("join agent");
    }
}
