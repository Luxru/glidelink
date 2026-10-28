# GlideLink Architecture

English | [Chinese](ARCHITECTURE.md)

See [README](README.en.md) for installation and usage.

## 1. Overview

GlideLink uses a controller / agent model:

- **Controller**: connected to the physical mouse, captures input, and decides when to forward it based on screen edges.
- **Agent**: receives input and injects it into the local system.
- **Tauri UI**: handles configuration and status only; it is not part of the input hot path.

The same application supports both roles.

## 2. Components and threads

```
┌──────────────────────────── Tauri process ────────────────────────────┐
│                                                                       │
│  Desktop UI ──commands──► Engine                                      │
│                            │                                          │
│              ┌─────────────┴─────────────┐                            │
│              │                           │                            │
│       Controller role             Agent role                          │
│       ┌──────────────┐            ┌──────────────┐                    │
│       │ Input capture│            │ Input inject │                    │
│       └──────┬───────┘            └──────▲───────┘                    │
│              │ channel                    │ channel                   │
│       ┌──────▼───────┐            ┌──────┴───────┐                    │
│       │ Controller   │◄── UDP ───►│ Agent        │                    │
│       │ network      │            │ network      │                    │
│       └──────────────┘            └──────────────┘                    │
└───────────────────────────────────────────────────────────────────────┘
```

Thread responsibilities:

- **Capture thread**: installs the system mouse hook and maintains edge state and peer coordinates.
- **Network thread**: handles handshakes, encrypted sending, heartbeats, reconnection, and status updates.
- **Injection thread**: runs only on the agent and turns network state into system mouse events.
- **Status object**: written only on state transitions to keep the hot path lock-free.

## 3. Data path

Controller:

1. The system-level hook receives mouse events.
2. When the pointer reaches the configured edge, forwarding mode starts.
3. Mouse movement is converted to normalized coordinates for the peer.
4. Move events are coalesced to the latest position in the lock-free channel; buttons and wheel are queued immediately.
5. The network thread encrypts and sends at an adaptive interval.

Agent:

1. The network thread receives and decrypts the packet.
2. It validates the session and replay sequence.
3. Coordinates and button state are passed to the injection thread.
4. The injection thread moves the pointer, clicks, or scrolls using OS APIs.

UDP is used instead of TCP/QUIC because pointer position is replaceable state: a newer position can discard an older one, and it should not be delayed by retransmission or head-of-line blocking. Button state is included in every input packet and reset on reconnect.

## 4. Coordinates and screen edges

Each machine's virtual desktop is mapped to `[0,1] × [0,1]`. The controller normalizes against its own virtual desktop; the agent maps the result to its own desktop. This keeps movement proportional across different resolutions.

Four directions are currently supported: the peer may be placed to the left, right, above, or below the controller. The peer can also slide along the shared edge; that offset is normalized to the host edge and shifts the entry/return mapping while the wire protocol stays unchanged.

- The pointer enters the peer after reaching an edge and continuing outward.
- The remote pointer starts just inside the edge to avoid jitter causing an immediate return.
- The remote pointer returns to the controller after crossing the return threshold.
- A short cooldown prevents rapid re-entry.

Injection uses absolute coordinates to avoid accumulated acceleration, DPI scaling, and packet-loss errors.

## 5. Input backends

### Windows

- A `WH_MOUSE_LL` low-level hook observes all mouse events.
- In forwarding mode, local events are consumed so both computers do not react at once.
- Raw Input provides relative movement.
- `mouse_event` performs absolute moves, button actions, and wheel injection.
- Controlling elevated windows requires the app to run elevated as well.

### macOS

- `CGEventTap` captures and suppresses mouse events at the HID layer.
- `CGEventPost` injects mouse events.
- Accessibility and Input Monitoring permissions are required.
- When permission is missing, the backend returns an error and the UI directs the user to System Settings.

## 6. Transport protocol

### Handshake

1. The agent binds the selected interface, picks a free port from 45890–45999, and generates a 20-character code: IPv4, port, a 4-byte random token, and a CRC8 checksum.
2. The controller decodes the code into the target IP/port, generates a random session ID, starts SPAKE2 (role A) with the full code, and sends HELLO.
3. The agent starts SPAKE2 (role B) with the same code and replies with ACCEPT.
4. Both sides derive directional session keys from the PAKE secret, session ID, and the complete HELLO/ACCEPT transcript.
5. The controller sends an encrypted confirmation and the agent acknowledges it; the session starts only after both confirmations succeed.

The code is generated by the agent backend and only shown in memory/status; it is not written to the config file. PAKE makes passive brute force infeasible, and the agent rate-limits handshake attempts with a token bucket.

### Encryption

- Key agreement: SPAKE2 over Ed25519 with a one-time pairing code.
- Session key derivation: HMAC-SHA256 bound to the session ID and handshake transcript.
- Data encryption: ChaCha20-Poly1305.
- Separate keys are used for controller → agent and agent → controller.
- Every packet carries a sequence number and authentication tag; the receiver uses a sliding replay window.

### Packet contents

Each mouse input packet contains:

- Normalized X/Y.
- Mouse button bitmask.
- Wheel delta.

A keyboard state packet is a fixed 34 bytes: a 32-byte USB HID usage bitmap, `ACTIVE` / `CAPS_LOCK` / `EVENT` flags, and a repeat-key HID. Caps Lock is a latched bit outside the held bitmap; repeats are marked separately. Key packets are ordered by session sequence so stale packets cannot resurrect released keys. HELLO/ACCEPT carry a platform identifier, which the controller uses for optional Cmd ↔ Ctrl adaptation.

The controller heartbeats every 100ms and smooths RTT/RTTVAR per RFC 6298. The agent derives RFC 3550-style expected/received packet loss from sequence numbers and returns it in PONG. Keyboard state is also sent immediately on every change and refreshed every 100ms to heal lost key-ups and Caps toggles; on timeout the controller stops forwarding and re-handshakes, and the agent releases all keys and mouse buttons. Keys already held before entering the remote stay local and must be released and pressed again to forward.

## 7. Security model

- The 20-character one-time code carries the IP, port, and token, and is used only for SPAKE2; session keys come from the PAKE output.
- The agent rate-limits handshake attempts, and the UDP port should only be reachable on a trusted LAN.
- Keyboard capture/forwarding is enabled only while the pointer is remote, key content is never logged, and all remote keys are released when returning local or disconnecting.
- Secure Input, elevated windows, and OS-reserved shortcuts can block capture or injection.
- The current version has no pinned device keys, long-term pairing-key rotation, or independent security audit.

## 8. Performance design

- The hot path avoids the WebView and Tauri IPC; the status lock is written only on transitions.
- Moves are coalesced to the latest position at a fixed 2ms; mouse buttons, wheel, and keyboard state are immediate. The agent collapses backlogged pure moves while preserving button, wheel, keyboard, and reset ordering; modifiers are pressed first and released last, and repeats are injected separately.
- The controller uses a UDP `connect` probe to let the OS choose its egress path. During a session, Windows uses 1ms timer resolution and high input-thread priority, macOS uses an `NSProcessInfo` activity plus QoS, and UDP is marked DSCP EF best-effort.
- Packets are small, unicast UDP with no retransmission; keys use physical HID usages and the remote layout determines characters. The agent injects only changed positions, mouse buttons, wheel deltas, and keyboard state.
- The target is 3–8ms end-to-end on a healthy LAN; actual results depend on the network and OS scheduling.

## 9. Future work

Current limitations are listed in the [README](README.en.md). Future work:

- Multiple agents and arbitrary rectangular layouts.
- Numpad, media keys, and IME-specific handling.
- LAN discovery and long-term pairing keys.
- Batched `SendInput` injection on Windows.
- Apple Silicon validation, signing/notarization, tray support, launch-at-login, and installers.
