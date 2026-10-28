# GlideLink

English | [Chinese](README.md)

Control two computers with one mouse and keyboard. Move the pointer off the screen edge to switch computers; move it back to return.
No switcher, drivers, or cloud account — input stays on your local network.

![GlideLink](screenshot.png)

## Features

- One mouse and keyboard for two computers, switching at the screen edge.
- Mouse movement, left/right/middle buttons, X1/X2, and wheel.
- Keyboard follows the mouse; optional automatic Cmd / Ctrl mapping between macOS and Windows.
- Four screen-edge directions, draggable peer arrangement, and an offset along the shared edge.
- Encrypted session established by a 20-character pairing code over LAN UDP.
- Chinese / English UI; Windows and macOS support.

## How it works

The controller captures mouse and keyboard input locally. When the pointer crosses the configured edge, input is encrypted and sent over the LAN to the controlled computer, which injects it. Moving back returns control and releases all remote keys. Nothing goes through the cloud and no kernel driver is required.

The UI is built with Tauri (Rust + WebView); settings are stored in `~/.glidelink/settings.json`.

## Requirements

- Rust 1.90 or newer.
- Windows 10/11 (WebView2) or macOS 12 or newer.
- Both computers on the same local network.

Node.js and a frontend build step are not required.

## Build and develop

```bash
cd src-tauri
cargo run              # development run
cargo test             # run tests
cargo build --release  # release build
```

The UI lives in `ui/` and is plain HTML / CSS / JS; rebuild to pick up changes.

## Usage

1. **Controlled PC**: select **Controlled PC**, choose a network interface in **Settings**, click **Start listening**, and send the generated pairing code to the controller.
2. **Controller**: select **Controller**, enter the pairing code, place the peer on a screen side in **Layout**, and click **Start controlling**.
3. Move the mouse across that edge to control the other computer; the keyboard follows. Move back across the edge to return.

## macOS permissions

Grant Accessibility and Input Monitoring in System Settings → Privacy & Security. Follow the in-app prompt, then reopen the app.

## Configuration

Settings are stored in your home directory:

- Windows: `C:\Users\<you>\.glidelink\settings.json`
- macOS: `~/.glidelink/settings.json`

The file holds the language, Cmd / Ctrl mapping, interface, role, and screen layout. The old `config.json` is migrated once on first launch.

## Current limitations

- One controlled PC only; standard keyboards only (no numpad, media keys, or IME-specific handling).
- Controlling elevated windows requires GlideLink to run elevated as well.
- The macOS input backend has not yet been verified on real hardware.
