# GlideLink

[English](README.en.md) | 中文

仓库：[Luxru/glidelink](https://github.com/Luxru/glidelink)

GlideLink 是一个使用 Rust + Tauri 实现的软件 KVM。它让一套鼠标在两台电脑之间无缝切换，不需要额外的切换器或驱动。

## 功能

- 一个物理鼠标控制两台电脑。
- 鼠标移动、左/右键、中键、X1/X2 和滚轮。
- 四方向屏幕边缘切换，并可沿共享边缘调整偏移；像第二块显示器一样拖动被控端屏幕来排列。
- 选择网卡后生成一个自包含配对码，内含本机 IPv4、自动选择的端口和一次性验证 token；PAKE 协商密钥。
- 局域网 UDP 实时传输，按 RTT/抖动自适应输入发送频率。
- 中文 / English 界面。
- Windows 与 macOS 支持。

## 工作原理

控制端捕获鼠标，越过屏幕边缘后经局域网加密转发给被控端注入；反向越过同一边缘返回。输入不经过云端，也不需要内核驱动。

## 环境要求

- Rust 1.90 或更新版本。
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

1. 选择「被控端」，在下拉框中选择要使用的网卡；应用会立即自动生成 20 位配对码（5 组 × 4 位）。
2. 点击「开始监听」；应用会在配对码包含的端口上开始监听。
3. 允许系统防火墙在专用网络上放行该应用的 UDP 连接。

### 2. 启动控制端

1. 选择「控制端」。
2. 输入或粘贴被控端显示的 20 位配对码（支持整段粘贴）。
3. 在「屏幕布局」中把被控端屏幕拖到本机的某一侧；沿边缘拖动可以调整偏移。
4. 点击「开始控制」。

配对码包含被控端 IP、端口和验证 token；因此只需要传输这一个码，不需要另外填写地址或端口。

连接成功后，将鼠标移出共享边缘即可控制被控端。反向越过同一条边缘返回本机。

### 3. 切换界面语言

界面左下角提供中文 / English 切换。选择会保存在本机；首次启动默认跟随系统语言。

## macOS 权限

macOS 需要授予以下权限：

- 系统设置 → 隐私与安全性 → 辅助功能（Accessibility）
- 系统设置 → 隐私与安全性 → 输入监控（Input Monitoring）

点击应用中的「打开系统设置」会触发系统授权提示，并打开仍缺少权限的设置面板；授权后点击「重新打开」重启应用。macOS 将授权与可执行文件绑定，因此未签名的开发版本在重新编译后可能需要再次授权。在 Windows 上，要控制以管理员权限运行的窗口，GlideLink 也需要以管理员身份运行。

## 配置

配置文件位置：

- Windows：`%APPDATA%\io.glidelink.desktop\config.json`
- macOS：`~/Library/Application Support/io.glidelink.desktop/config.json`

配置文件保存角色、所选网卡 IP 和布局；一次性配对码不会写入磁盘。

## 安全与延迟

- 20 位配对码包含 IPv4、端口、4 字节随机 token 和校验位；通过 SPAKE2 PAKE 协商会话密钥，ChaCha20-Poly1305 加密，双向独立密钥和防重放窗口。
- 配对码仅限局域网且限制尝试速率；不要把 UDP 端口映射到公网。
- 无云端、无账号；正常局域网下目标输入反馈为 3–8ms。控制端每 100ms 心跳，按 RFC 6298 估计 RTT/抖动，并显示 RFC 3550 式接收端丢包率。
- 鼠标移动固定 2ms 合帧；按键和滚轮立即发送。被控端会合并积压的纯移动，但保留每次按键、滚轮和断开处理。
- 控制端通过操作系统选路确定出口；会话期间 Windows 提升定时器分辨率与输入线程优先级，macOS 阻止 App Nap 并提升 QoS；UDP 尽力标记 DSCP EF。
- PAKE 与协议尚未经过独立安全审计；详细设计见架构文档。

## 状态与限制

- Windows 后端已通过 `x86_64-pc-windows-msvc` 类型检查；macOS 捕获/注入已在 macOS 13.6（Intel）通过 `--self-test`。真实 Windows ↔ macOS 双机、Apple Silicon、签名和公证仍未验证。
- 协议 v2 不做旧版本兼容；两台电脑必须使用同一版本。
- 仅支持 IPv4；配对码固定为 20 位，包含地址、端口和 token。
- 自动端口范围是 `45890–45999`；控制端不需要填写端口。
- 目前仅支持鼠标；键盘转发尚未实现。
- 只支持一个被控端；被控端会吸附到四个边缘之一，并可沿该边缘设置偏移。尚不支持任意二维位置、间隙或重叠。
- 屏幕位置需要手动配置，尚未实现自动发现。
- 被控端以管理员权限运行的应用可能无法接收注入事件。
- 尚未提供签名安装包、托盘和开机自启动。

## 开发与测试

```bash
cd glidelink/src-tauri
cargo test
cargo build
```

测试覆盖 SPAKE2 握手、加密、防重放、坐标映射，以及真实 UDP 回环下的握手和输入转发。

在 macOS 上授权后，可以通过已签名的 app bundle 运行自检，让 TCC 归属到 GlideLink：

```bash
open -n -W -o /tmp/glidelink-selftest.out --stderr /tmp/glidelink-selftest.err \
  src-tauri/target/release/bundle/macos/GlideLink.app --args --self-test
cat /tmp/glidelink-selftest.out
```

它会检查显示器与 TCC 权限，模拟边缘穿越（捕获）并移动光标（注入），结束后恢复光标位置。直接运行 `cargo run -- --self-test` 时，TCC 可能归属到终端而不是 GlideLink，因此可能报告权限缺失。

更多设计说明见 [ARCHITECTURE.md](ARCHITECTURE.md) / [ARCHITECTURE.en.md](ARCHITECTURE.en.md)。

## 许可证

本项目使用 GNU General Public License v3.0 only（GPL-3.0-only）。完整文本见 [LICENSE](LICENSE)。
