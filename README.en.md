# GlideLink

English | [Chinese](README.md)

Control multiple computers with one mouse and keyboard. The controller listens and agents join with one shared pairing code; move the pointer across freely arranged screens, including directly between agents. LAN direct, no drivers or accounts.

![GlideLink](screenshot.png)

## Features

- The controller listens; up to 8 agents can join.
- Relative mouse motion, buttons, wheel, and keyboard forwarding; touching edges allow direct traversal, including agent-to-agent.
- Free 2D draggable arrangement of multiple agent screens, persisted per device.
- Keyboard follows the mouse; optional macOS ⌘ ↔ Windows Ctrl mapping.
- Encrypted sessions via one 20-character controller pairing code over LAN UDP.
- Keyboard & shortcuts settings are controller-only and include a fixed release shortcut.
- A Logs page lets you choose Error/Warn/Info/Debug/Trace and copy the log buffer.
- Chinese / English; Windows and macOS.

## Build

```bash
cd src-tauri
cargo run              # development
cargo test             # tests
cargo build --release  # release
```

Node.js and a frontend build step are not required.

## Usage

1. Controller: select **Controller** and pick the listening interface in **Settings**. The pairing code is generated automatically; click **Start listening** and copy it.
2. Agent: select **Controlled PC**, enter that code, and click **Connect**. Multiple agents use the same code.
3. In **Display Layout**, drag agent screens. When edges touch, the pointer can move directly from the host or from another agent.
4. Press `Ctrl + Alt + Backspace` to force control back to the local machine.

The controller needs a LAN-reachable UDP listening port; agents only need outbound access to it. Grant Accessibility and Input Monitoring in macOS System Settings → Privacy & Security.

> The pairing code is a bearer secret shared by all agents. Use it only on trusted LANs and limit who can see it.

Settings live in `~/.glidelink/settings.json` (Windows: `C:\Users\<you>\.glidelink\settings.json`).
