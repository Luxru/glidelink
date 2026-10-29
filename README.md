# GlideLink

[English](README.en.md) | 中文

一套键鼠控制多台电脑，支持 Windows 和 macOS。最多连接 8 台被控端，通过加密 TCP 在局域网直连，无需驱动或账号。

![GlideLink](screenshot.png)

## 使用

1. 控制端选择监听网卡，点击「开始监听」，复制 20 位配对码。
2. 被控端输入配对码并连接；多台设备使用同一个码。
3. 在「布局」中排列屏幕，边缘相接即可移动鼠标切换，键盘随鼠标转发。
4. 按 `Ctrl + Alt + Backspace` 强制返回本机。

控制端需放行 TCP 监听端口；macOS 需授予辅助功能与输入监控权限。配对码等同连接凭证，请只在可信局域网使用并妥善保管。

设置保存在 `~/.glidelink/settings.json`（Windows：`C:\Users\<用户>\.glidelink\settings.json`）。日志可在界面中查看、调整级别和复制。

## 开发

```bash
cd src-tauri
cargo run
cargo test
cargo build --release
```

无需 Node.js 或前端构建步骤。后端设计见 [架构说明](ARCHITECTURE.md)。
