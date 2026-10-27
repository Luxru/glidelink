# GlideLink

English | [Chinese](README.md)

Repository: [Luxru/glidelink](https://github.com/Luxru/glidelink)

GlideLink is a software KVM built with Rust and Tauri. It lets one mouse move seamlessly between two computers without extra hardware or drivers.

## Features

- One physical mouse controlling two computers.
- Mouse movement, left/right/middle buttons, X1/X2, and wheel.
- Four screen-edge directions with an adjustable offset along the shared edge; drag the peer screen to arrange it like a second display.
- Low-latency LAN UDP transport with encryption, heartbeats, and automatic reconnection.
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

1. Select **Controlled PC**.
2. Set the listening port (default: `45890`).
3. Enter a pairing code, or click **Regenerate**.
4. Click **Start listening**.
5. Allow the UDP port on private networks in the system firewall.
6. Note this computer's LAN IP address.

### 2. Start the controller

1. Select **Controller**.
2. Enter the peer address, for example `192.168.1.42:45890`.
3. Enter exactly the same pairing code.
4. Open **Display layout** and drag the peer screen to the side of this device; slide it along the edge to set the offset.
5. Click **Start controlling**.

Once connected, move the mouse across the shared edge to control the peer. Cross the same edge in the opposite direction to return.

### 3. Change the language

Use the Chinese / English selector in the lower-left corner of the app. The choice is stored locally. On first launch, the language follows the system locale.

## macOS permissions

macOS requires:

- System Settings → Privacy & Security → Accessibility
- System Settings → Privacy & Security → Input Monitoring

Click **Open System Settings** in the app to show the system prompt and open the pane for the permission that is still missing. Relaunch the app after granting permissions. macOS associates these grants with the built executable, so an unsigned development build may need to be granted again after it is rebuilt. On Windows, GlideLink must also run as administrator if you need to control elevated windows.

## Configuration

Configuration file locations:

- Windows: `%APPDATA%\io.glidelink.desktop\config.json`
- macOS: `~/Library/Application Support/io.glidelink.desktop/config.json`

The file contains the pairing code. Keep it readable only by your user account.

## Security and latency

- PBKDF2-HMAC-SHA256 derives keys; ChaCha20-Poly1305 encrypts traffic with directional keys and a replay window.
- The pairing code is the only authentication secret. Use it only on a trusted LAN and never expose the UDP port to the internet.
- No cloud or accounts. The target is 3–8ms of input feedback on a healthy LAN; the in-app RTT is network round-trip time only.
- The protocol has not had an independent security audit. See the architecture document for details.

## Status and limitations

- The Windows backend type-checks for `x86_64-pc-windows-msvc`; macOS capture and injection pass `--self-test` on macOS 13.6 (Intel). A real Windows ↔ macOS pair, Apple Silicon, signing, and notarization are still unverified.
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

Tests cover the protocol, encryption, replay protection, coordinate mapping, and a real UDP loopback handshake and input-forwarding path.

On macOS, after granting permissions, verify the input backend without starting the UI:

```bash
cargo run -- --self-test
```

It reports displays and TCC permissions, exercises a synthetic edge crossing (capture), moves the cursor to a known point (injection), and restores the cursor. Run it from the terminal or app that holds the permissions.

See [ARCHITECTURE.md](ARCHITECTURE.md) / [ARCHITECTURE.en.md](ARCHITECTURE.en.md) for more details.

## License

This project is licensed under the GNU General Public License v3.0 only (GPL-3.0-only). See [LICENSE](LICENSE).
