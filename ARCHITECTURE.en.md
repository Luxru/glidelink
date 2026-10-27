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
4. State is handed to the network thread through a lock-free channel.
5. The network thread encrypts and sends one UDP packet.

Agent:

1. The network thread receives and decrypts the packet.
2. It validates the session and replay sequence.
3. Coordinates and button state are passed to the injection thread.
4. The injection thread moves the pointer, clicks, or scrolls using OS APIs.

UDP is used instead of TCP/QUIC because pointer position is replaceable state: a newer position can discard an older one, and it should not be delayed by retransmission or head-of-line blocking. Button state is included in every input packet and reset on reconnect.

## 4. Coordinates and screen edges

Each machine's virtual desktop is mapped to `[0,1] × [0,1]`. The controller normalizes against its own virtual desktop; the agent maps the result to its own desktop. This keeps movement proportional across different resolutions.

Four directions are currently supported: the peer may be placed to the left, right, above, or below the controller.

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

1. The controller generates a random session ID and sends HELLO.
2. Both sides derive a master key from the pairing code and session ID.
3. The agent verifies the HELLO HMAC and replies with ACCEPT.
4. Both sides derive directional data keys.

### Encryption

- Key derivation: PBKDF2-HMAC-SHA256, 100,000 rounds.
- Data encryption: ChaCha20-Poly1305.
- Separate keys are used for controller → agent and agent → controller.
- Every packet carries a sequence number and authentication tag; the receiver uses a sliding replay window.

### Packet contents

Each input packet contains:

- Normalized X/Y.
- Button bitmask.
- Wheel delta.

The controller sends a heartbeat every 250ms and measures RTT. On timeout it stops forwarding and re-handshakes. The agent releases held buttons and returns to the waiting state when its session times out.

## 7. Security model

- Traffic is encrypted with keys derived from the pairing code.
- No accounts, cloud services, or internet access are involved.
- The pairing code is the only authentication secret and must be sufficiently random.
- The UDP port should only be reachable on a trusted LAN.
- The current version has no pinned device public keys, key rotation, or independent security audit.

## 8. Performance design

- The input hot path avoids the WebView and Tauri IPC.
- The status lock is written only on state transitions.
- Input packets are small and use unicast UDP without congestion control or retransmission overhead.
- The agent injects movement only when coordinates change, avoiding idle event noise.
- The target is 3–8ms end-to-end on a healthy LAN. Actual results depend on the mouse, Wi-Fi, OS scheduling, and power settings.

## 9. Known limitations

- Mouse only; keyboard forwarding is not implemented.
- One agent is supported.
- Four edge directions only; arbitrary display layouts are not implemented yet.
- Peer addresses are entered manually; there is no automatic discovery.
- The Windows secure desktop (UAC) cannot be captured.
- The macOS backend still needs validation on macOS hardware.
- No signed installers, tray integration, or launch-at-login.
- The protocol has not undergone an independent security audit.

## 10. Future work

- Keyboard forwarding.
- Multiple agents and arbitrary rectangular display layouts.
- LAN discovery and pairing.
- Batched `SendInput` injection on Windows.
- macOS hardware validation, signing, and notarization.
- Tray support, launch-at-login, and installers.
