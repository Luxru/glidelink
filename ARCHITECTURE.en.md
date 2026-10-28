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

The controller captures mouse and keyboard at the system level. When the pointer reaches the configured edge and keeps moving outward, forwarding starts: mouse movement is coalesced to the latest position, while buttons, wheel, and keyboard state are sent immediately. The agent decrypts and injects the input. Crossing the return threshold restores local control and releases all remote keys.

Coordinates are normalized to 0–1 on each virtual desktop, so movement stays proportional across resolutions. The peer can sit to the left, right, above, or below, and can slide along the shared edge.

Input uses UDP: pointer position is replaceable state, so old packets need no retransmission. Button state travels with every packet and resets after reconnect.

## Platform backends

- **Windows**: low-level mouse/keyboard hooks for capture, Raw Input for movement, `mouse_event` for injection.
- **macOS**: `CGEventTap` for capture, `CGEventPost` for injection; Accessibility and Input Monitoring permissions are required.

## Transport

The agent generates a 20-character pairing code containing its IP, port, and a random token. Both sides run SPAKE2 to derive session keys, encrypt every packet with ChaCha20-Poly1305, and reject replays by sequence number. The controller sends a 100ms heartbeat to measure round-trip time and packet loss.

Ports are chosen automatically from 45890–45999 and should only be reachable on the local network.

## Configuration

All settings are read and written by the Rust backend and stored in `~/.glidelink/settings.json`. The old `config.json` is migrated once on first launch and is never read again.
