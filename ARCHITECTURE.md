# GlideLink 架构

[English](ARCHITECTURE.en.md) | 中文

使用与编译见 [README.md](README.md)。

## 角色与拓扑

- **控制端（Controller Hub）**：监听 UDP 端口，捕获本机鼠标/键盘，维护多台被控端会话和 2D 屏幕布局。
- **被控端（Agent）**：连接控制端，接收输入并注入本机系统。

控制端生成一个共享的 20 位配对码（本机 IP、端口、随机 token）。所有 Agent 使用同一个码连入，最多 8 台。Controller 只负责输入策略，Agent 只负责执行。

## 组件

```
ui/ ──命令──► Engine
                ├── 控制端：输入捕获 → 2D 目标路由 → 多会话 UDP 发送
                └── 被控端：UDP 接收 → 输入注入
```

- `input/`：系统级输入捕获与注入；捕获消息带目标 device id。
- `net/`：Controller Hub、Agent 客户端、握手、加密、心跳、重连、多会话。
- `layout.rs`：2D 世界坐标、屏幕矩形、目标切换、布局持久化。
- `config.rs`、`state.rs`：设置、设备身份、布局和运行时状态。
- `ui/`：HTML / CSS / JS 界面，无前端构建步骤。

## 设备身份与握手

- 每次安装生成并持久化 16 字节 `device_id`，握手携带。
- Agent → Controller `HELLO`，Controller → Agent `ACCEPT`，Agent → Controller `CONFIRM`，Controller → Agent `CONFIRM_ACK`。
- 每台设备独立 SID、SPAKE2 会话密钥和重放窗口；`HELLO_LEN = 88`。
- Controller 为每个 Agent 维护独立会话、RTT、丢包、心跳、pacer 和 reconcile；一个 Agent 断开不影响其他 Agent。
- 同 device id 重连会替换旧会话并沿用原布局位置。

## 2D 布局与输入路由

世界坐标以控制端屏幕宽度为 1 单位；每块屏幕宽度 1.0，高度由分辨率宽高比决定。

- Host 固定在 `(0,0)`，Agent 屏幕由用户自由拖动。
- 指针连续维护一个世界坐标；进入某个在线 Agent 矩形即切换目标。
- Host ↔ Agent、Agent ↔ Agent 都支持直接穿越；屏幕间有间隙时视为墙，不能跨越。
- 返回 Host 时显示光标并 warp 到对应位置；在 Agent 之间切换时光标保持隐藏。
- 目标切换时旧 Agent 收到 release-all/inactive，新 Agent 收到当前鼠标键和 Caps 状态；切屏时仍按住的键盘键需要重新按下。
- 活动设备断线或布局移除时自动返回 Host。`Ctrl + Alt + Backspace` 可强制释放并返回。
- 布局保存在 `config.layout`，新设备按空闲位置自动放置；监听中且指针不在远端时可以拖动并立即生效。

## 输入事件

- 鼠标移动：相对像素位移 `dx/dy`，2ms 内累加后发送，并携带当前鼠标按键状态（`TYPE_INPUT`，12 字节）。进入/切换屏幕时额外发送一次 `TYPE_WARP` 绝对位置用于对齐光标。
- 位移按目标屏幕分辨率缩放，保持跨分辨率移动比例一致。
- 鼠标按键、滚轮、键盘按下/抬起、Caps Lock：离散事件立即发送（`TYPE_EVENT`）。
- 被控端负责按键重复（500ms 首延迟、32ms 间隔）和双击计数；控制端不转发系统 autorepeat。
- 每个 Agent 每 500ms 收到一次 reconcile：活动目标收到活动状态，其余收到 inactive；丢包造成的卡键最多在一个周期后自愈。
- 瞬时敲击或相对位移在极端丢包下仍可能丢失；进入屏幕时的 warp 会重新对齐光标。

## 平台实现

- **Windows**：低层鼠标/键盘钩子捕获，Raw Input 读取移动，`mouse_event`/`SendInput` 注入。
- **macOS**：`CGEventTap` 捕获，`CGEventPost` 注入；需要辅助功能与输入监控权限。Caps 通过 IOHID 设置锁定状态，避免误触输入法切换。

## 通信与安全

双方用 SPAKE2 协商每台设备的会话密钥，ChaCha20-Poly1305 加密每个包，序列号防重放。Controller 对活动目标 100ms、空闲 Agent 500ms 心跳；Agent 也主动发送 500ms PING。双方 PONG 携带接收统计，Controller 和 Agent 都显示 RTT、抖动和丢包。

端口在 45890–45999 中自动选择。Controller 必须允许局域网访问该 UDP 端口；Agent 只需主动连接。共享配对码是 bearer secret，只应在可信局域网使用；限制设备数量和 HELLO 速率，不记录配对码。

## 配置

设置由 Rust 后端读写，保存在 `~/.glidelink/settings.json`：角色、监听接口、device id、布局、最近配对码、语言、键位映射和日志级别。运行时日志同时写入 stderr 和内存环形缓冲，供 UI 日志面板查看和复制。
