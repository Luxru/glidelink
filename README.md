# GlideLink

[English](README.en.md) | 中文

仓库：[Luxru/glidelink](https://github.com/Luxru/glidelink)

GlideLink 是一个使用 Rust + Tauri 实现的软件 KVM。它让一套鼠标在两台电脑之间无缝切换，不需要额外的切换器或驱动。

## 功能

- 一个物理鼠标控制两台电脑。
- 鼠标移动、左/右键、中键、X1/X2 和滚轮。
- 四方向屏幕边缘切换：被控端可以位于左侧、右侧、上方或下方。
- 局域网 UDP 实时传输，带加密、心跳和自动重连。
- 中文 / English 界面。
- Windows 与 macOS 支持。

## 工作原理

控制端捕获本机鼠标事件。当鼠标越过设定的屏幕边缘时，控制端把事件通过局域网发送给被控端；被控端将鼠标事件注入自己的系统。反向越过同一条边缘即可返回本机。

输入数据不经过云端，不访问互联网。程序不需要安装内核驱动，也不改变网络配置。

## 环境要求

- Rust 1.77 或更新版本。
- Windows 10/11（需要 WebView2）。
- macOS 12 或更新版本（需要 Xcode Command Line Tools）。
- 两台电脑位于同一局域网，能够互相访问 UDP 端口。

不需要 Node.js 或前端构建工具。

## 快速开始

### 构建与运行

```bash
cd glidelink/src-tauri
cargo run
```

发布构建：

```bash
cd glidelink/src-tauri
cargo build --release
```

如果 crates.io 下载较慢，可以临时使用镜像：

```bash
cargo run --config 'source.crates-io.replace-with="ustc"' \
  --config 'source.ustc.registry="sparse+https://mirrors.ustc.edu.cn/crates.io-index/"'
```

### 1. 启动被控端

1. 选择「被控端」。
2. 设置监听端口，默认 `45890`。
3. 填写配对码，或点击「重新生成」。
4. 点击「开始监听」。
5. 允许系统防火墙在专用网络上放行该 UDP 端口。
6. 查看本机局域网 IP。

### 2. 启动控制端

1. 选择「控制端」。
2. 填写被控端地址，例如 `192.168.1.42:45890`。
3. 填写完全相同的配对码。
4. 在「屏幕布局」中选择被控端位于本机的哪一侧。
5. 点击「开始控制」。

连接成功后，将鼠标移出所选屏幕边缘即可控制被控端。反向越过同一条边缘返回本机。

### 3. 切换界面语言

界面左下角提供中文 / English 切换。选择会保存在本机；首次启动默认跟随系统语言。

## macOS 权限

macOS 需要授予以下权限：

- 系统设置 → 隐私与安全性 → 辅助功能（Accessibility）
- 系统设置 → 隐私与安全性 → 输入监控（Input Monitoring）

授权后重新启动应用。在 Windows 上，要控制以管理员权限运行的窗口，GlideLink 也需要以管理员身份运行。

## 配置

配置文件位置：

- Windows：`%APPDATA%\io.glidelink.desktop\config.json`
- macOS：`~/Library/Application Support/io.glidelink.desktop/config.json`

配置文件包含配对码。请确保它只对当前用户可读。

## 安全说明

- 配对码通过 PBKDF2-HMAC-SHA256 派生会话密钥。
- 输入数据使用 ChaCha20-Poly1305 加密。
- 每个方向使用独立密钥，并带序列号和防重放窗口。
- 无云端、无账号、无外部网络依赖。
- 配对码是唯一认证凭据。请使用生成的高熵配对码，不要把 UDP 端口映射到公网。
- 当前协议尚未经过独立安全审计。

## 延迟

目标是正常局域网下 3–8ms 的输入反馈。实际延迟取决于鼠标上报率、Wi-Fi 质量、系统调度和电源策略。界面中的 RTT 是网络往返时间，不等于完整端到端延迟。

蓝牙不适合实时鼠标通道，因此 GlideLink 使用局域网 UDP。

## 状态与限制

- Windows 输入后端已在本机编译并完成启动/停止冒烟测试。
- macOS 输入后端已通过 macOS 目标的类型检查，但尚未在 macOS 实机运行验证。
- 目前仅支持鼠标；键盘转发尚未实现。
- 目前只支持一个被控端和四方向边缘切换。
- 屏幕位置需要手动配置，尚未实现自动发现。
- 被控端以管理员权限运行的应用可能无法接收注入事件。
- 尚未提供签名安装包、托盘和开机自启动。

## 开发与测试

```bash
cd glidelink/src-tauri
cargo test
cargo build
```

测试覆盖协议、加密、防重放、坐标映射，以及真实 UDP 回环下的握手和输入转发。

更多设计说明见 [ARCHITECTURE.md](ARCHITECTURE.md) / [ARCHITECTURE.en.md](ARCHITECTURE.en.md)。

## 许可证

本项目使用 GNU General Public License v3.0 only（GPL-3.0-only）。完整文本见 [LICENSE](LICENSE)。
