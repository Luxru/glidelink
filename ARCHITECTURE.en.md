# GlideLink Architecture

English | [Chinese](ARCHITECTURE.md)

See the [README](README.en.md) for usage and build instructions.

## Roles

One application supports both roles, selected in Settings:

- **Controller**: captures the local mouse and keyboard and forwards input when the pointer crosses a screen edge.
- **Agent**: receives input and injects it into the local system.

The Tauri UI only handles configuration and status; it is not part of input processing.

## Components

```
ui/ ──commands──► Engine
                    ├── Controller: input capture → network send (UDP)
                    └── Agent: network receive → input injection
```

- `input/`: system-level input capture and injection, implemented per platform.
- `net/`: pairing, handshake, encryption, UDP transport, heartbeats, reconnection.
- `layout.rs`: screen edges and coordinate mapping.
- `config.rs`, `state.rs`: settings persistence and runtime state.
- `ui/`: HTML / CSS / JS interface with no frontend build step.

## Input flow

The controller captures mouse and keyboard at the system level. When the pointer reaches the configured edge and keeps moving outward, forwarding starts:

- Mouse movement is coalesced to the latest position (`TYPE_INPUT`, absolute normalized coordinates).
- Mouse buttons, wheel, key down/up, and Caps Lock are discrete events sent immediately (`TYPE_EVENT`), so coalescing a move cannot drop a click or key edge.
- The agent owns key repeat (500 ms delay, 32 ms interval) and double-click counting; the controller does not forward OS autorepeat.
- Every 500 ms the controller sends `TYPE_RECONCILE` with held keys, held buttons, Caps Lock, and whether the pointer is remote. A lost event heals within one period; disconnect or timeout releases everything.
- Crossing the return threshold restores local control and sends release-all plus an inactive state.

Coordinates are normalized to 0–1 on each virtual desktop, so movement stays proportional across resolutions. The peer can sit to the left, right, above, or below, and can slide along the shared edge.

Input uses UDP: pointer position is replaceable state, so old packets need no retransmission. Discrete events are not retransmitted; the 500 ms reconciliation repairs recoverable state. A very short tap can still be lost on an extreme lossy link.

## Platform backends

- **Windows**: low-level mouse/keyboard hooks for capture, Raw Input for movement, `mouse_event` for injection.
- **macOS**: `CGEventTap` for capture, `CGEventPost` for injection; Accessibility and Input Monitoring permissions are required.

## Transport

The agent generates a 20-character pairing code containing its IP, port, and a random token. Both sides run SPAKE2 to derive session keys, encrypt every packet with ChaCha20-Poly1305, and reject replays by sequence number. The controller sends a 100ms heartbeat to measure round-trip time and packet loss.

Ports are chosen automatically from 45890–45999 and should only be reachable on the local network.

## Configuration

All settings are read and written by the Rust backend and stored in `~/.glidelink/settings.json`.
