# GlideLink

[English](README.en.md) | 中文

一套鼠标和键盘控制两台电脑。鼠标移出屏幕边缘就切到另一台，移回来就回到本机。局域网直连，无需驱动或账号。

![GlideLink](screenshot.png)

## 功能

- 鼠标移动、按键、滚轮与键盘转发，越过屏幕边缘切换。
- 键盘跟随鼠标；可自动对应 macOS 的 ⌘ 与 Windows 的 Ctrl。
- 上下左右四个方向，可拖动排列被控端屏幕并沿边缘调整偏移。
- 20 位配对码建立加密会话，局域网 UDP 直连。
- 中文 / English，支持 Windows 与 macOS。

## 编译

```bash
cd src-tauri
cargo run              # 开发运行
cargo test             # 运行测试
cargo build --release  # 发布构建
```

不需要 Node.js 或前端构建工具。

## 使用

1. 被控端：选择「被控端」，在「设置」选网卡，点击「开始监听」，把配对码发给对方。
2. 控制端：选择「控制端」，输入配对码，在「布局」中把被控端放到屏幕某一侧，点击「开始控制」。
3. 鼠标移出该边缘即可控制另一台电脑，键盘同时跟随。

macOS 需在系统设置中授予辅助功能与输入监控权限。

设置保存在 `~/.glidelink/settings.json`（Windows：`C:\Users\<用户>\.glidelink\settings.json`）。
