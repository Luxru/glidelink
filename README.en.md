# GlideLink

English | [Chinese](README.md)

Repository: [Luxru/glidelink](https://github.com/Luxru/glidelink)

GlideLink is a software KVM built with Rust and Tauri. It lets one mouse move seamlessly between two computers without extra hardware or drivers.

## Features

- One physical mouse controlling two computers.
- Mouse movement, left/right/middle buttons, X1/X2, and wheel.
- Four screen-edge directions: the peer can be on the left, right, top, or bottom.
- Low-latency LAN UDP transport with encryption, heartbeats, and automatic reconnection.
- Chinese / English UI.
- Windows and macOS support.

## How it works

The controller captures local mouse events. When the pointer crosses the configured screen edge, it sends the events over the local network to the controlled computer, which injects them into its own input system. Crossing the same edge in the opposite direction returns control to the local computer.

Input traffic never goes through a cloud service or the internet. GlideLink does not require kernel drivers and does not change network configuration.

## Requirements

- Rust 1.77 or newer.
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
4. Choose which side the peer is on under **Display layout**.
5. Click **Start controlling**.

Once connected, move the mouse across the chosen screen edge to control the peer. Cross the same edge in the opposite direction to return.

### 3. Change the language

Use the Chinese / English selector in the lower-left corner of the app. The choice is stored locally. On first launch, the language follows the system locale.

## macOS permissions

macOS requires:

- System Settings → Privacy & Security → Accessibility
- System Settings → Privacy & Security → Input Monitoring

Relaunch the app after granting permissions. On Windows, GlideLink must also run as administrator if you need to control elevated windows.

## Configuration

Configuration file locations:

- Windows: `%APPDATA%\io.glidelink.desktop\config.json`
- macOS: `~/Library/Application Support/io.glidelink.desktop/config.json`

The file contains the pairing code. Keep it readable only by your user account.

## Security

- The pairing code is used with PBKDF2-HMAC-SHA256 to derive session keys.
- Input traffic is encrypted with ChaCha20-Poly1305.
- Each direction has its own key, packet sequence numbers, and a replay window.
- No cloud, no accounts, and no internet dependency.
- The pairing code is the only authentication secret. Use a generated high-entropy code, and never expose the UDP port to the internet.
- The protocol has not yet undergone an independent security audit.

## Latency

The target is 3–8ms of input feedback on a healthy LAN. Actual latency depends on the mouse polling rate, Wi-Fi quality, OS scheduling, and power settings. The RTT shown in the app is the network round-trip time, not full end-to-end latency.

Bluetooth is not suitable for a real-time mouse channel, so GlideLink uses UDP over the LAN.

## Status and limitations

- The Windows input backend is built and smoke-tested locally.
- The macOS input backend has been type-checked for a macOS target but has not yet been run on macOS hardware.
- Mouse only; keyboard forwarding is not implemented.
- One peer and four edge directions only.
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

See [ARCHITECTURE.md](ARCHITECTURE.md) / [ARCHITECTURE.en.md](ARCHITECTURE.en.md) for more details.

## License

This project is licensed under the GNU General Public License v3.0 only (GPL-3.0-only). See [LICENSE](LICENSE).
