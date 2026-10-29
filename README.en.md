# GlideLink

English | [Chinese](README.md)

Control up to 8 agents with one mouse and keyboard on Windows and macOS. Connections use encrypted TCP over the LAN, with no drivers or accounts.

![GlideLink](screenshot.png)

## Usage

1. Controller: choose a network interface, start listening, and copy the 20-character pairing code.
2. Agent: enter the code and connect. All agents use the same code.
3. Arrange screens in **Display Layout**. Move across touching edges to switch devices; the keyboard follows the pointer.
4. Press `Ctrl + Alt + Backspace` to return control to the local machine.

Allow the controller's TCP listening port through the firewall. On macOS, grant Accessibility and Input Monitoring permissions. Treat the pairing code as a connection credential and use it only on trusted LANs.

Settings live in `~/.glidelink/settings.json` (Windows: `C:\Users\<you>\.glidelink\settings.json`). The UI provides log viewing, level selection, and copying.

## Development

```bash
cd src-tauri
cargo run
cargo test
cargo build --release
```

No Node.js or frontend build step is needed. See the [architecture](ARCHITECTURE.en.md) for backend details.
