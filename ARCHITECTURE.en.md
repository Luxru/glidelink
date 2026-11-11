# rx-switch Architecture

[Chinese](ARCHITECTURE.md) | [Usage and build](README.en.md)

Application 0.1.0, protocol v1, unreleased. The controller captures and routes input; agents receive and inject it. Device identity and layout persist, while each connection gets a fresh session.

## Input

- Both world axes use the controller's screen width as their unit. Each screen is 1 unit wide, with height determined by its aspect ratio.
- Pointer messages carry normalized absolute positions at 250 Hz, falling back to 125 Hz under high RTT or RTT variation.
- Positions, semantic events, and state snapshots share one input model. Adjacent positions may merge; every other message first commits preceding movement, preserving click, drag, and key order.
- State snapshots run every 500 ms. Switching releases the old target and transfers mouse buttons and Caps state; held keyboard keys must be pressed again.
- Disconnects and session errors release input and return control locally. Reconnect starts inactive, and old session commands cannot reach its replacement.
- Windows uses low-level capture hooks and absolute `SendInput`; Raw Input only detects outward motion at the host edge. Tauri uses the `Always` device event filter so the capture thread owns Raw Input registration and window focus does not interfere with keyboard hooks. macOS uses `CGEventTap` and `CGEventPost`, authorized by Accessibility.
- Intentional Caps Lock presses are distinct from lock-state repair. When Caps Lock input-source switching is enabled on macOS, TIS selects the remembered ASCII / non-ASCII source on the main thread. The injector waits for completion with cancellation on shutdown. Reconciliation only repairs lock state; ordinary lock changes post native `FlagsChanged` events.
- macOS keyboard, mouse, and wheel injection share a private event source and explicit modifier flags. Modifier releases are constructed as key-up before conversion to `FlagsChanged`, preventing mouse clicks from inheriting stale Command state after the keyboard has released it.

## Transport

Each agent uses one TCP connection with `TCP_NODELAY`. SPAKE2 derives keys, ChaCha20-Poly1305 encrypts messages, and contiguous sequence numbers verify order.

Frames contain a big-endian `u16` length followed by a protocol packet, limited to 1024 bytes. The handshake is `HELLO → ACCEPT → CONFIRM → CONFIRM_ACK`; input is processed only after authentication.

The controller sends heartbeats every 100 ms to the active target and every 500 ms to other agents. Agents send every 500 ms; 3 seconds without messages ends the connection. Send queues have capacity and timeout limits. Logs report queue state and injection timing; the UI shows RTT and RTT variation, without estimating IP packet loss.

Initial agent connection attempts share a 5-second window. After an authenticated link disconnects, input is released on detection and recovery lasts up to 30 seconds, with a 1-second delay after each failed attempt. TCP connection and handshake waits share the same deadline; failed attempts never extend it. The user can stop from the window or tray at any time. Expiry ends the session, preserves an actionable error, and reaps old workers so a new pairing code can be used.

TCP loss can delay input during retransmission; position merging only reduces application backlog. Listening ports are chosen from 45890–45999. Both endpoints use the same build.

## Code

- `input/`: platform capture, shared input messages, and injection.
- `net/`: sessions, handshake, TCP framing, encryption, and heartbeat.
- `layout.rs`: world coordinates, routing, and layout.
- `config.rs`, `state.rs`: settings and runtime lifecycle.
- `tray.rs`: tray menus, window restoration, and menu language.
- `ui/`: HTML, CSS, and JavaScript interface.
