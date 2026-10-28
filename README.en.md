# GlideLink

English | [Chinese](README.md)

Repository: [Luxru/glidelink](https://github.com/Luxru/glidelink)

GlideLink is a software KVM built with Rust and Tauri. It lets one mouse move seamlessly between two computers without extra hardware or drivers.

## Features

- One physical mouse controlling two computers.
- Mouse movement, left/right/middle buttons, X1/X2, and wheel.
- Four screen-edge directions with an adjustable offset along the shared edge; drag the peer screen to arrange it like a second display.
- Pick a network interface and get a self-contained code containing the local IPv4, an automatically chosen port, and a one-time token; SPAKE2 PAKE establishes the session.
- Low-latency LAN UDP with RTT/jitter-adaptive input pacing.
- Chinese / English UI.
- Windows and macOS support.

## How it works

The controller captures the mouse and, after the pointer crosses a screen edge, sends encrypted input over the LAN to the peer, which injects it. Crossing the same edge in the opposite direction returns control. No cloud service or kernel driver is involved.

## Requirements

- Rust 1.90 or newer.
- Windows 10/11 with WebView2.
- macOS 12 or newer with Xcode Command Line Tools.
- Both computers on the same LAN, able to reach each other over UDP.

Node.js and a frontend build step are not required.

## Getting started

### Build and run

```bash
cd glidelink/src-tauri
cargo run
```

Release build:

```bash
cd glidelink/src-tauri
cargo build --release
```

If crates.io is slow, you can use a mirror temporarily:

```bash
cargo run --config 'source.crates-io.replace-with="ustc"' \
  --config 'source.ustc.registry="sparse+https://mirrors.ustc.edu.cn/crates.io-index/"'
```

### 1. Start the controlled computer

1. Select **Controlled PC** and choose a network interface; the app immediately generates a 20-character code (5 groups of 4).
2. Click **Start listening**; the app listens on the port carried by the code.
3. Allow the app's UDP traffic on private networks in the system firewall.

### 2. Start the controller

1. Select **Controller**.
2. Enter or paste the 20-character code shown on the controlled computer (whole-code paste is supported).
3. Open **Display layout** and drag the peer screen to the side of this device; slide it along the edge to set the offset.
4. Click **Start controlling**.

The code carries the peer IP, port, and verification token, so no address or port needs to be entered separately.

Once connected, move the mouse across the shared edge to control the peer. Cross the same edge in the opposite direction to return.

### 3. Change the language

Use the Chinese / English selector in the lower-left corner of the app. The choice is stored locally. On first launch, the language follows the system locale.

## macOS permissions

macOS requires:

- System Settings → Privacy & Security → Accessibility
- System Settings → Privacy & Security → Input Monitoring

Click **Open System Settings** in the app to show the system prompt and open the pane for the permission that is still missing. After granting permissions, click **Reopen** to restart the app. macOS associates these grants with the built executable, so an unsigned development build may need to be granted again after it is rebuilt. On Windows, GlideLink must also run as administrator if you need to control elevated windows.

## Configuration

Configuration file locations:

- Windows: `%APPDATA%\io.glidelink.desktop\config.json`
- macOS: `~/Library/Application Support/io.glidelink.desktop/config.json`

The file stores the role, selected interface IP, and layout; the one-time pairing code is never written to disk.

## Security and latency

- A 20-character code carries IPv4, port, a 4-byte random token, and a checksum; SPAKE2 PAKE derives session keys, and ChaCha20-Poly1305 encrypts traffic with directional keys and a replay window.
- The code is LAN-only and handshake attempts are rate-limited. Never expose the UDP port to the internet.
- No cloud or accounts. The target is 3–8ms of input feedback on a healthy LAN. The controller heartbeats every 100ms, estimates RTT/jitter with RFC 6298, and shows RFC 3550-style receiver packet loss.
- Mouse movement is coalesced at a fixed 2ms; buttons and wheel are sent immediately. The agent collapses backlogged pure moves while preserving every button, wheel, and reset transition.
- The controller lets the OS choose its egress path. During a session, Windows raises timer resolution and input-thread priority, macOS prevents App Nap and raises QoS, and UDP is marked DSCP EF on a best-effort basis.
- The PAKE exchange and protocol have not had an independent security audit. See the architecture document for details.

## Status and limitations

- The Windows backend type-checks for `x86_64-pc-windows-msvc`; macOS capture and injection pass `--self-test` on macOS 13.6 (Intel). A real Windows ↔ macOS pair, Apple Silicon, signing, and notarization are still unverified.
- Protocol v2 does not support older builds; both computers must run the same version.
- The automatic port range is `45890–45999`; the controller never needs to enter a port.
- IPv4 only. The code is always 20 characters because it carries the address, port, and token.
- Mouse only; keyboard forwarding is not implemented.
- One peer; the peer snaps to one of four edges with an offset along that edge. Arbitrary 2D placement, gaps, and overlap are not implemented.
- Display placement is configured manually; there is no automatic discovery yet.
- Applications running with elevated privileges may not receive injected events.
- Signed installers, tray support, and launch-at-login are not included yet.

## Development and tests

```bash
cd glidelink/src-tauri
cargo test
cargo build
```

Tests cover the SPAKE2 handshake, encryption, replay protection, coordinate mapping, and a real UDP loopback handshake and input-forwarding path.

On macOS, after granting permissions, run the self-test through the signed app bundle so TCC is attributed to GlideLink:

```bash
open -n -W -o /tmp/glidelink-selftest.out --stderr /tmp/glidelink-selftest.err \
  src-tauri/target/release/bundle/macos/GlideLink.app --args --self-test
cat /tmp/glidelink-selftest.out
```

It reports displays and TCC permissions, exercises a synthetic edge crossing (capture), moves the cursor to a known point (injection), and restores the cursor. Running `cargo run -- --self-test` directly may attribute TCC to the terminal instead of GlideLink and report missing permissions.

See [ARCHITECTURE.md](ARCHITECTURE.md) / [ARCHITECTURE.en.md](ARCHITECTURE.en.md) for more details.

## License

This project is licensed under the GNU General Public License v3.0 only (GPL-3.0-only). See [LICENSE](LICENSE).
