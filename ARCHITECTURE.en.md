# GlideLink Architecture

English | [Chinese](ARCHITECTURE.md)

See the [README](README.en.md) for usage and build instructions.

## Roles and topology

- **Controller Hub**: listens on UDP, captures the local mouse/keyboard, and maintains multiple agent sessions plus a 2D screen layout.
- **Agent**: connects to the controller, receives input, and injects it locally.

The controller generates one shared 20-character pairing code (local IP, port, random token). All agents use the same code; up to 8 may join. The controller owns input policy; agents only execute.

## Components

```
ui/ ──commands──► Engine
                    ├── Controller: capture → 2D target routing → multi-session UDP
                    └── Agent: UDP receive → input injection
```

- `input/`: system-level capture/injection; capture messages carry a target device id.
- `net/`: controller hub, agent client, handshake, encryption, heartbeat, reconnect, multi-session.
- `layout.rs`: 2D world coordinates, screen rectangles, target transitions, layout persistence.
- `config.rs`, `state.rs`: settings, device identity, layout, runtime state.
- `ui/`: HTML / CSS / JS interface with no frontend build step.

## Device identity and handshake

- Each installation generates and persists a 16-byte `device_id`, carried in the handshake.
- Agent → Controller `HELLO`, Controller → Agent `ACCEPT`, Agent → Controller `CONFIRM`, Controller → Agent `CONFIRM_ACK`.
- Every device gets its own SID, SPAKE2 session keys, and replay window; `HELLO_LEN = 88`.
- The controller keeps per-agent sessions, RTT, loss, heartbeat, pacer, and reconcile. One agent disconnecting does not affect the others.
- Reconnecting with the same device id replaces the old session and keeps its layout position.

## 2D layout and input routing

World units use the controller screen width as 1. Every screen is 1.0 wide and its height follows the screen aspect ratio.

- The host is fixed at `(0,0)`; agent screens are freely draggable.
- The pointer keeps a continuous world coordinate; entering an online agent rectangle switches target.
- Host ↔ agent and agent ↔ agent traversal are direct. Gaps between screens act as walls.
- Returning to the host shows the cursor and warps it to the corresponding point; switching between agents keeps the cursor hidden.
- On target switch the old agent receives release-all/inactive; the new one receives current buttons and Caps state; keyboard keys still held during the switch must be pressed again.
- If the active device disconnects or leaves the layout, control returns to the host. `Ctrl + Alt + Backspace` forces release.
- Layout lives in `config.layout`; new devices are auto-placed. While listening with the pointer local, dragging updates the live world and persists.

## Input events

- Mouse movement: relative pixel deltas `dx/dy`, accumulated over 2 ms (`TYPE_INPUT`, 8 bytes). A one-off `TYPE_WARP` absolute position aligns the cursor on entry or target switch.
- Deltas are scaled to the target screen resolution so movement stays proportional across resolutions.
- Mouse buttons, wheel, key down/up, and Caps Lock: discrete events (`TYPE_EVENT`).
- The agent owns key repeat (500 ms / 32 ms) and double-click counting; the controller does not forward OS autorepeat.
- Every agent receives a 500 ms reconcile: the active target gets active state, others get inactive. Lost releases heal within one period.
- A very short tap or relative motion can still be lost on an extreme lossy link; the entry warp realigns the cursor.

## Platform backends

- **Windows**: low-level mouse/keyboard hooks, Raw Input for movement, `mouse_event`/`SendInput` for injection.
- **macOS**: `CGEventTap` for capture, `CGEventPost` for injection; Accessibility and Input Monitoring permissions are required. Caps Lock is set through IOHID to avoid accidentally switching input sources.

## Transport and security

Both sides run SPAKE2 per device; ChaCha20-Poly1305 encrypts every packet and sequence numbers reject replays. The controller pings the active target every 100 ms and idle agents every 500 ms; PONG carries receive statistics for RTT and loss.

Ports are chosen from 45890–45999. The controller must allow LAN access to that UDP port; agents only connect outbound. The shared pairing code is a bearer secret; use it only on trusted LANs, keep the device count bounded, rate-limit HELLO, and never log the code.

## Configuration

Settings are read and written by the Rust backend in `~/.glidelink/settings.json`: role, listening interface, device id, layout, last pairing code, language, and modifier mapping.
