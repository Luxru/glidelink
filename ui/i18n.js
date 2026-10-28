(() => {
  const dictionaries = {
    zh: {
      "app.taglineShort": "软件 KVM",
      "sidebar.settings": "设置",
      "sidebar.secureLocal": "仅局域网 · 端到端加密",
      "nav.session": "会话",
      "nav.layout": "屏幕布局",
      "nav.about": "关于",
      "language.label": "语言",
      "language.zh": "中文",
      "language.en": "English",

      "page.session.title": "会话",
      "page.session.subtitle.controller": "连接并控制另一台电脑",
      "page.session.subtitle.agent": "接受另一台电脑的鼠标控制",
      "page.layout.title": "屏幕布局",
      "page.layout.subtitle": "像多显示器一样排列两台电脑",
      "page.about.title": "关于",
      "page.about.subtitle": "GlideLink 版本与会话统计",

      "status.notRunning": "未运行",
      "status.waiting": "等待连接",
      "status.connected": "已连接",
      "status.controlling": "正在控制",
      "status.controlled": "受控中",
      "status.error": "异常",
      "status.startFailed": "启动失败",
      "status.notStarted": "未启动",
      "status.waitingPeer": "等待对端",

      "common.stop": "停止",
      "common.notConnected": "未连接",
      "common.waitingConnection": "等待连接",
      "common.waitingController": "等待控制端",
      "common.thisDevice": "本机",
      "common.peerControlled": "被控端",
      "common.peerController": "控制端",
      "common.copyPasscode": "复制配对码",

      "error.title": "无法开始",
      "permission.title": "需要 macOS 辅助功能与输入监控权限",
      "permission.text": "允许 GlideLink 监听和注入鼠标事件，然后重新启动应用。",
      "permission.open": "打开系统设置",
      "permission.reopen": "重新打开",

      "session.lede": "选择这台电脑的角色，两台设备使用相同的配对码建立一条仅限局域网的会话。",
      "roles.group": "设备角色",
      "role.controller": "控制端",
      "role.controllerDesc": "这台电脑连接鼠标，并控制另一台电脑",
      "role.agent": "被控端",
      "role.agentDesc": "这台电脑接受另一台电脑的鼠标控制",

      "connection.group": "连接",
      "connection.interface": "网络接口",
      "connection.localIp": "本机 IP",
      "connection.layout": "屏幕布局",
      "connection.passcode": "配对码",

      "status.group": "状态",
      "status.localScreen": "本机屏幕",
      "status.peer": "对端",
      "status.rtt": "网络往返",

      "button.startControl": "开始控制",
      "button.startListening": "开始监听",
      "button.running": "会话运行中",
      "button.connecting": "正在连接…",
      "button.listening": "正在监听…",

      "toast.startedController": "会话已启动，等待对端响应",
      "toast.startedAgent": "正在监听局域网连接",
      "toast.stopped": "会话已停止",
      "toast.passcodeCopied": "配对码已复制",
      "toast.passcodeCopyFailed": "复制失败，请手动选择配对码",
      "toast.passcodeEmpty": "配对码为空",

      "validation.pairingCode": "配对码无效，请重新输入完整邀请码。",
      "validation.interface": "请选择网络接口。",

      "layout.lede": "把被控端屏幕放在本机的某一侧。鼠标越过对应边缘后会进入另一台电脑，反向移动即可返回。",
      "layout.group": "排列",
      "layout.groupNote": "像多显示器一样使用",
      "layout.previewWaiting": "等待连接",
      "layout.previewReady": "链路已就绪",
      "layout.previewForwarding": "输入转发中",
      "layout.previewReceiving": "接收输入中",
      "layout.peerAriaLabel": "被控端屏幕，可拖动排列。方向键选择方位，Shift 加方向键调整偏移。",
      "layout.summaryLeft": "被控端在左侧",
      "layout.summaryRight": "被控端在右侧",
      "layout.summaryTop": "被控端在上方",
      "layout.summaryBottom": "被控端在下方",
      "layout.summaryAgent": "由控制端决定",

      "about.tagline": "低延迟软件 KVM",
      "about.version": "版本 0.1.0 · Alpha",
      "about.sessionGroup": "会话",
      "about.role": "角色",
      "about.state": "状态",
      "about.duration": "时长",
      "about.peer": "对端",
      "about.linkState": "连接",
      "about.linkGroup": "链路",
      "about.rtt": "往返延迟",
      "about.jitter": "抖动",
      "about.loss": "丢包率",
      "about.inputRate": "输入间隔",
      "about.localScreen": "本机屏幕",
      "about.remoteScreen": "对端屏幕",
      "about.trafficGroup": "会话流量",
      "about.packetsSent": "发送包数",
      "about.packetsReceived": "接收包数",
      "about.bytesSent": "发送数据",
      "about.bytesReceived": "接收数据",

      "errors.withDetail": "{message}（{detail}）",
      "errors.UNKNOWN": "操作失败",
      "errors.COMMAND_TIMEOUT": "后台服务响应超时，请检查程序是否正常运行。",
      "errors.PERMISSIONS_BLOCKED": "macOS 尚未授予辅助功能或输入监控权限，无法监听或注入鼠标事件。",
      "errors.E_PAIRING_CODE": "配对码无效，请重新输入完整邀请码。",
      "errors.E_LOCAL_IP": "请选择有效的网络接口。",
      "errors.E_PORT_RANGE": "没有可用的本地端口，请重试。",
      "errors.E_CONFIG_READ": "读取配置文件失败。",
      "errors.E_CONFIG_PARSE": "配置文件格式损坏。",
      "errors.E_CONFIG_WRITE": "写入配置文件失败。",
      "errors.E_SESSION_RUNNING": "会话已经在运行。",
      "errors.E_INTERNAL": "内部状态异常。",
      "errors.E_IPV6_UNSUPPORTED": "当前版本仅支持 IPv4 地址。",
      "errors.E_NET_BIND": "创建或绑定 UDP 套接字失败。",
      "errors.E_NET_SOCKET": "设置网络套接字失败。",
      "errors.E_NET_THREAD": "启动网络线程失败。",
      "errors.E_CAPTURE_THREAD": "启动鼠标捕获线程失败。",
      "errors.E_CAPTURE_TIMEOUT": "鼠标捕获初始化超时。",
      "errors.E_VIRTUAL_SCREEN": "无法读取虚拟桌面尺寸。",
      "errors.E_MODULE_HANDLE": "无法获取程序模块句柄。",
      "errors.E_CAPTURE_WINDOW": "创建隐藏输入窗口失败。",
      "errors.E_RAW_INPUT": "注册 Raw Input 失败。",
      "errors.E_MOUSE_HOOK": "安装全局鼠标钩子失败。",
      "errors.E_CAPTURE_TIMER": "创建输入状态定时器失败。",
      "errors.E_INJECTOR_THREAD": "启动输入注入线程失败。",
      "errors.E_INJECTOR_TIMEOUT": "输入注入初始化超时。",
      "errors.E_MAC_PERMISSIONS_CAPTURE": "macOS 未授予辅助功能或输入监控权限。",
      "errors.E_MAC_PERMISSIONS_INJECT": "macOS 未授予辅助功能权限，无法注入鼠标事件。",
      "errors.E_MAC_DISPLAYS": "无法读取 macOS 显示器布局。",
      "errors.E_MAC_EVENT_TAP": "无法创建鼠标事件 Tap。",
      "errors.E_MAC_RUNLOOP": "无法创建事件 Tap 的 RunLoop Source。",
      "errors.E_OPEN_SETTINGS": "打开系统设置失败。",
      "errors.E_UNSUPPORTED": "当前平台不受支持。",
      "errors.NET_SEND_FAILED": "网络发送失败，正在重连。",
      "errors.NET_NO_RESPONSE": "未收到对端响应，请检查地址、配对码和防火墙设置。",
      "errors.NET_TIMEOUT": "连接超时，正在重连。",
      "errors.NET_PEER_DISCONNECTED": "对端已断开，等待重新连接。",
      "errors.CONFIG_LOAD_FAILED": "加载配置失败。",
    },

    en: {
      "app.taglineShort": "Software KVM",
      "sidebar.settings": "Settings",
      "sidebar.secureLocal": "LAN only · End-to-end encrypted",
      "nav.session": "Session",
      "nav.layout": "Display Layout",
      "nav.about": "About",
      "language.label": "Language",
      "language.zh": "Chinese",
      "language.en": "English",

      "page.session.title": "Session",
      "page.session.subtitle.controller": "Connect and control another computer",
      "page.session.subtitle.agent": "Accept mouse control from another computer",
      "page.layout.title": "Display Layout",
      "page.layout.subtitle": "Arrange both computers like multiple displays",
      "page.about.title": "About",
      "page.about.subtitle": "GlideLink version and session statistics",

      "status.notRunning": "Not running",
      "status.waiting": "Waiting for peer",
      "status.connected": "Connected",
      "status.controlling": "Controlling",
      "status.controlled": "Under control",
      "status.error": "Error",
      "status.startFailed": "Failed to start",
      "status.notStarted": "Not started",
      "status.waitingPeer": "Waiting for peer",

      "common.stop": "Stop",
      "common.notConnected": "Not connected",
      "common.waitingConnection": "Waiting for connection",
      "common.waitingController": "Waiting for controller",
      "common.thisDevice": "This device",
      "common.peerControlled": "Controlled PC",
      "common.peerController": "Controller",
      "common.copyPasscode": "Copy pairing code",

      "error.title": "Could not start",
      "permission.title": "macOS Accessibility and Input Monitoring required",
      "permission.text": "Allow GlideLink to listen for and inject mouse events, then relaunch the app.",
      "permission.open": "Open System Settings",
      "permission.reopen": "Reopen",

      "session.lede": "Choose this computer's role. Both devices use the same pairing code to create a local-only session.",
      "roles.group": "Device role",
      "role.controller": "Controller",
      "role.controllerDesc": "This computer has the mouse and controls the other one",
      "role.agent": "Controlled PC",
      "role.agentDesc": "This computer is controlled by the other one",

      "connection.group": "Connection",
      "connection.interface": "Network interface",
      "connection.localIp": "Local IP",
      "connection.layout": "Display layout",
      "connection.passcode": "Pairing code",

      "status.group": "Status",
      "status.localScreen": "This screen",
      "status.peer": "Peer",
      "status.rtt": "Network RTT",

      "button.startControl": "Start controlling",
      "button.startListening": "Start listening",
      "button.running": "Session running",
      "button.connecting": "Connecting…",
      "button.listening": "Listening…",

      "toast.startedController": "Session started, waiting for the peer",
      "toast.startedAgent": "Listening for a LAN connection",
      "toast.stopped": "Session stopped",
      "toast.passcodeCopied": "Pairing code copied",
      "toast.passcodeCopyFailed": "Copy failed; select the pairing code manually",
      "toast.passcodeEmpty": "Pairing code is empty",

      "validation.pairingCode": "The invite code is incomplete or invalid.",
      "validation.interface": "Select a network interface.",

      "layout.lede": "Place the peer screen on one side of this computer. Move the mouse across that edge to enter the other computer; move back across the same edge to return.",
      "layout.group": "Arrangement",
      "layout.groupNote": "Use like multiple displays",
      "layout.previewWaiting": "Waiting for connection",
      "layout.previewReady": "Link ready",
      "layout.previewForwarding": "Forwarding input",
      "layout.previewReceiving": "Receiving input",
      "layout.peerAriaLabel": "Peer screen. Drag to arrange it. Arrow keys choose a side; Shift plus arrows adjust the offset.",
      "layout.summaryLeft": "Peer on the left",
      "layout.summaryRight": "Peer on the right",
      "layout.summaryTop": "Peer above",
      "layout.summaryBottom": "Peer below",
      "layout.summaryAgent": "Set by the controller",

      "about.tagline": "Low-latency software KVM",
      "about.version": "Version 0.1.0 · Alpha",
      "about.sessionGroup": "Session",
      "about.role": "Role",
      "about.state": "State",
      "about.duration": "Duration",
      "about.peer": "Peer",
      "about.linkState": "Link",
      "about.linkGroup": "Link",
      "about.rtt": "Round-trip time",
      "about.jitter": "Jitter",
      "about.loss": "Packet loss",
      "about.inputRate": "Input interval",
      "about.localScreen": "This screen",
      "about.remoteScreen": "Peer screen",
      "about.trafficGroup": "Session traffic",
      "about.packetsSent": "Packets sent",
      "about.packetsReceived": "Packets received",
      "about.bytesSent": "Data sent",
      "about.bytesReceived": "Data received",

      "errors.withDetail": "{message} ({detail})",
      "errors.UNKNOWN": "Something went wrong",
      "errors.COMMAND_TIMEOUT": "The background service did not respond. Check that the app is running.",
      "errors.PERMISSIONS_BLOCKED": "macOS Accessibility or Input Monitoring permission is missing, so mouse events cannot be captured or injected.",
      "errors.E_PAIRING_CODE": "The invite code is incomplete or invalid.",
      "errors.E_LOCAL_IP": "Select a valid network interface.",
      "errors.E_PORT_RANGE": "No local port is available. Try again.",
      "errors.E_CONFIG_READ": "Could not read the configuration file.",
      "errors.E_CONFIG_PARSE": "The configuration file is malformed.",
      "errors.E_CONFIG_WRITE": "Could not write the configuration file.",
      "errors.E_SESSION_RUNNING": "A session is already running.",
      "errors.E_INTERNAL": "Internal state error.",
      "errors.E_IPV6_UNSUPPORTED": "This version supports IPv4 only.",
      "errors.E_NET_BIND": "Could not create or bind the UDP socket.",
      "errors.E_NET_SOCKET": "Could not configure the network socket.",
      "errors.E_NET_THREAD": "Could not start the network thread.",
      "errors.E_CAPTURE_THREAD": "Could not start the mouse capture thread.",
      "errors.E_CAPTURE_TIMEOUT": "Mouse capture initialization timed out.",
      "errors.E_VIRTUAL_SCREEN": "Could not read the virtual desktop size.",
      "errors.E_MODULE_HANDLE": "Could not get the application module handle.",
      "errors.E_CAPTURE_WINDOW": "Could not create the hidden input window.",
      "errors.E_RAW_INPUT": "Could not register Raw Input.",
      "errors.E_MOUSE_HOOK": "Could not install the global mouse hook.",
      "errors.E_CAPTURE_TIMER": "Could not create the input state timer.",
      "errors.E_INJECTOR_THREAD": "Could not start the input injection thread.",
      "errors.E_INJECTOR_TIMEOUT": "Input injection initialization timed out.",
      "errors.E_MAC_PERMISSIONS_CAPTURE": "macOS Accessibility or Input Monitoring permission is missing.",
      "errors.E_MAC_PERMISSIONS_INJECT": "macOS Accessibility permission is missing, so mouse events cannot be injected.",
      "errors.E_MAC_DISPLAYS": "Could not read the macOS display layout.",
      "errors.E_MAC_EVENT_TAP": "Could not create the mouse event tap.",
      "errors.E_MAC_RUNLOOP": "Could not create the event tap run-loop source.",
      "errors.E_OPEN_SETTINGS": "Could not open System Settings.",
      "errors.E_UNSUPPORTED": "This platform is not supported.",
      "errors.NET_SEND_FAILED": "Network send failed. Reconnecting.",
      "errors.NET_NO_RESPONSE": "No response from the peer. Check the address, pairing code, and firewall.",
      "errors.NET_TIMEOUT": "Connection timed out. Reconnecting.",
      "errors.NET_PEER_DISCONNECTED": "The peer disconnected. Waiting to reconnect.",
      "errors.CONFIG_LOAD_FAILED": "Could not load the configuration.",
    },
  };

  const supported = ["zh", "en"];
  const storageKey = "glidelink.locale";

  function detectLocale() {
    try {
      const saved = localStorage.getItem(storageKey);
      if (supported.includes(saved)) return saved;
    } catch {
      // ignore storage failures
    }
    const language = (navigator.language || "en").toLowerCase();
    return language.startsWith("zh") ? "zh" : "en";
  }

  let locale = detectLocale();

  function translate(key, vars) {
    let value = dictionaries[locale]?.[key];
    if (value == null) value = dictionaries.en[key];
    if (value == null) return key;
    if (vars) {
      for (const [name, replacement] of Object.entries(vars)) {
        value = value.replaceAll(`{${name}}`, String(replacement));
      }
    }
    return value;
  }

  function apply(root = document) {
    root.documentElement?.setAttribute("lang", locale === "zh" ? "zh-CN" : "en");
    const scope = root.querySelectorAll ? root : document;
    scope.querySelectorAll("[data-i18n]").forEach((element) => {
      element.textContent = translate(element.dataset.i18n);
    });
    scope.querySelectorAll("[data-i18n-placeholder]").forEach((element) => {
      element.setAttribute("placeholder", translate(element.dataset.i18nPlaceholder));
    });
    scope.querySelectorAll("[data-i18n-title]").forEach((element) => {
      element.setAttribute("title", translate(element.dataset.i18nTitle));
    });
    scope.querySelectorAll("[data-i18n-aria-label]").forEach((element) => {
      element.setAttribute("aria-label", translate(element.dataset.i18nAriaLabel));
    });
    const select = document.getElementById("languageSelect");
    if (select) select.value = locale;
  }

  function setLocale(next) {
    if (!supported.includes(next) || next === locale) return;
    locale = next;
    try {
      localStorage.setItem(storageKey, locale);
    } catch {
      // ignore storage failures
    }
    apply();
    document.dispatchEvent(new CustomEvent("glidelink:locale", { detail: { locale } }));
  }

  window.glidelinkI18n = {
    t: translate,
    setLocale,
    getLocale: () => locale,
    apply,
    supported,
  };

  apply();
})();
