const invoke = window.__TAURI__?.core?.invoke;
const i18n = window.glidelinkI18n;
const t = (key, vars) => i18n.t(key, vars);

const els = {
  form: document.getElementById("configForm"),
  pageScroll: document.querySelector(".page-scroll"),
  pageTitle: document.getElementById("pageTitle"),
  navItems: [...document.querySelectorAll(".nav-item")],
  languageSelect: document.getElementById("languageSelect"),
  roleOptions: [...document.querySelectorAll(".role-option")],
  controllerFields: document.getElementById("controllerFields"),
  keyboardGroup: document.getElementById("keyboardGroup"),
  networkGroup: document.getElementById("networkGroup"),
  adaptModifiers: document.getElementById("adaptModifiers"),
  interfaceSelect: document.getElementById("interfaceSelect"),
  selectedIp: document.getElementById("selectedIp"),
  openNetworkSettings: document.getElementById("openNetworkSettings"),
  codeCells: [...document.querySelectorAll(".code-cell")],
  startBtn: document.getElementById("startBtn"),
  startLabel: document.getElementById("startLabel"),
  stopBtn: document.getElementById("stopBtn"),
  statusPill: document.getElementById("statusPill"),
  statusText: document.getElementById("statusText"),
  errorBanner: document.getElementById("errorBanner"),
  errorText: document.getElementById("errorText"),
  layoutStage: document.getElementById("layoutStage"),
  layoutWorld: document.getElementById("layoutWorld"),
  layoutEmpty: document.getElementById("layoutEmpty"),
  latencyDot: document.getElementById("latencyDot"),
  latencyValue: document.getElementById("latencyValue"),
  localScreenText: document.getElementById("localScreenText"),
  peerText: document.getElementById("peerText"),
  deviceCount: document.getElementById("deviceCount"),
  permissionCard: document.getElementById("permissionCard"),
  permissionBtn: document.getElementById("permissionBtn"),
  reopenBtn: document.getElementById("reopenBtn"),
  copyPasscode: document.getElementById("copyPasscode"),
  sessionState: document.getElementById("sessionState"),
  aboutRole: document.getElementById("aboutRole"),
  aboutState: document.getElementById("aboutState"),
  aboutDuration: document.getElementById("aboutDuration"),
  aboutPeer: document.getElementById("aboutPeer"),
  aboutDeviceCount: document.getElementById("aboutDeviceCount"),
  aboutPeerPlatform: document.getElementById("aboutPeerPlatform"),
  aboutLinkState: document.getElementById("aboutLinkState"),
  aboutRtt: document.getElementById("aboutRtt"),
  aboutJitter: document.getElementById("aboutJitter"),
  aboutLoss: document.getElementById("aboutLoss"),
  aboutRate: document.getElementById("aboutRate"),
  aboutLocalScreen: document.getElementById("aboutLocalScreen"),
  aboutRemoteScreen: document.getElementById("aboutRemoteScreen"),
  aboutPacketsSent: document.getElementById("aboutPacketsSent"),
  aboutPacketsReceived: document.getElementById("aboutPacketsReceived"),
  aboutBytesSent: document.getElementById("aboutBytesSent"),
  aboutBytesReceived: document.getElementById("aboutBytesReceived"),
  aboutBuild: document.getElementById("aboutBuild"),
  logLevel: document.getElementById("logLevel"),
  logPanel: document.getElementById("logPanel"),
  logCount: document.getElementById("logCount"),
  copyLogs: document.getElementById("copyLogs"),
  clearLogs: document.getElementById("clearLogs"),
  toast: document.getElementById("toast"),
};

const ui = {
  page: "session",
  role: "controller",
  running: false,
  loading: false,
  initialized: false,
  lastStatus: null,
  lastState: null,
  startError: null,
  interfaces: [],
  interfaceSignature: "",
  selectedIp: "",
  configLocalIp: "",
  lastPairingCode: "",
  lastListenCode: "",
  previewListenIp: "",
  previewListenPromise: null,
  devices: [],
  peers: [],
  targetDevice: null,
  language: i18n.getLocale(),
  adaptModifiers: false,
  savedConfig: null,
  languageSynced: false,
  languageSyncGuard: false,
  settingsSavePromise: null,
};

const WORLD_UNIT = 220;
const LAYOUT_PADDING = 28;
const LAYOUT_SNAP = 0.12;

let dragState = null;

const PAGES = {
  session: "page.session.title",
  layout: "page.layout.title",
  settings: "page.settings.title",
  logs: "page.logs.title",
  about: "page.about.title",
};

const PAIRING_CODE_LENGTH = 20;
const COMMAND_TIMEOUT_MS = 12000;

let toastTimer;

function normalizeCodePart(value, limit = PAIRING_CODE_LENGTH) {
  return String(value || "")
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "")
    .replace(/O/g, "0")
    .replace(/[IL]/g, "1")
    .slice(0, limit);
}

function getPairingCode() {
  return normalizeCodePart(els.codeCells.map((cell) => cell.value).join(""));
}

function setPairingCode(value) {
  const code = normalizeCodePart(value);
  els.codeCells.forEach((cell, index) => {
    cell.value = code.slice(index * 4, index * 4 + 4);
  });
}

function formatScreen(screen) {
  if (!screen || !screen.width || !screen.height) return "—";
  return `${screen.width} × ${screen.height}`;
}

function formatDuration(ms) {
  const total = Math.max(0, Math.floor(Number(ms) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}

function formatBytes(bytes) {
  const value = Math.max(0, Number(bytes) || 0);
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let scaled = value / 1024;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  return `${scaled >= 10 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[index]}`;
}

function formatCount(value) {
  return Math.max(0, Number(value) || 0).toLocaleString();
}

function sessionStateLabel() {
  const status = ui.lastStatus || {};
  if (!ui.running) return t("status.notRunning");
  if (status.remoteActive) {
    return ui.role === "agent" ? t("status.controlled") : t("status.controlling");
  }
  if (ui.role === "controller" && !status.connected) {
    return t("status.listening");
  }
  return status.connected ? t("status.connected") : t("status.waitingPeer");
}

function updateAboutStats(status) {
  const running = Boolean(status?.running);
  const connected = Boolean(status?.connected);
  const latency = status?.latencyMs == null ? null : Number(status.latencyMs);
  els.aboutRole.textContent = ui.role === "agent" ? t("role.agent") : t("role.controller");
  els.aboutState.textContent = sessionStateLabel();
  els.aboutDuration.textContent = running && status?.startedAtMs
    ? formatDuration(Date.now() - status.startedAtMs)
    : "—";
  els.aboutPeer.textContent = status?.peer || "—";
  els.aboutDeviceCount.textContent = String(Array.isArray(status?.devices) ? status.devices.length : 0);
  const peerPlatform = { 1: "platform.macos", 2: "platform.windows" }[status?.peerPlatform];
  els.aboutPeerPlatform.textContent = peerPlatform ? t(peerPlatform) : "—";
  els.aboutLinkState.textContent = connected
    ? t("status.connected")
    : running
      ? t("status.waiting")
      : t("status.notRunning");
  els.aboutRtt.textContent = latency == null ? "—" : `${latency.toFixed(1)} ms`;
  els.aboutJitter.textContent = status?.jitterMs == null
    ? "—"
    : `${Number(status.jitterMs).toFixed(1)} ms`;
  els.aboutLoss.textContent = status?.packetLossPercent == null
    ? "—"
    : `${Number(status.packetLossPercent).toFixed(1)} %`;
  els.aboutRate.textContent = status?.inputIntervalMs == null
    ? "—"
    : `${Number(status.inputIntervalMs).toFixed(1)} ms`;
  els.aboutLocalScreen.textContent = formatScreen(status?.localScreen);
  els.aboutRemoteScreen.textContent = formatScreen(status?.remoteScreen);
  els.aboutPacketsSent.textContent = formatCount(status?.packetsSent);
  els.aboutPacketsReceived.textContent = formatCount(status?.packetsReceived);
  els.aboutBytesSent.textContent = formatBytes(status?.bytesSent);
  els.aboutBytesReceived.textContent = formatBytes(status?.bytesReceived);
  els.aboutBuild.textContent = ui.lastState?.build || "—";
}

function errorToRaw(error) {
  if (typeof error === "string") return error;
  if (error?.message) return String(error.message);
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function translateError(raw) {
  if (!raw) return "";
  const value = String(raw);
  const divider = value.indexOf("|");
  const code = divider === -1 ? value : value.slice(0, divider);
  const detail = divider === -1 ? "" : value.slice(divider + 1);
  let message;
  if (code.includes(".")) {
    message = t(code);
    if (message === code) message = value;
  } else {
    message = t(`errors.${code}`);
    if (message === `errors.${code}`) {
      message = code.startsWith("E_") || code.startsWith("NET_") || code === "CONFIG_LOAD_FAILED"
        ? t("errors.UNKNOWN")
        : value;
    }
  }
  return detail ? t("errors.withDetail", { message, detail }) : message;
}

function showToast(message) {
  els.toast.textContent = message;
  els.toast.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => els.toast.classList.remove("show"), 1800);
}

function setError(raw) {
  ui.startError = raw || null;
  if (raw) {
    els.errorText.textContent = translateError(raw);
    els.errorBanner.hidden = false;
  } else {
    els.errorBanner.hidden = true;
    els.errorText.textContent = "";
  }
}

function updatePageChrome() {
  els.pageTitle.textContent = t(PAGES[ui.page] || PAGES.session);
}


function setPage(page) {
  if (!PAGES[page]) return;
  ui.page = page;
  for (const section of document.querySelectorAll(".page")) {
    section.hidden = section.dataset.page !== page;
  }
  for (const item of els.navItems) {
    const active = item.dataset.pageTarget === page;
    item.classList.toggle("active", active);
    if (active) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
  updatePageChrome();
  els.pageScroll.scrollTop = 0;
  if (page === "layout") requestAnimationFrame(renderLayout);
  if (page === "logs") refreshLogs();
}

function updateStartLabel() {
  if (ui.loading) {
    els.startLabel.textContent = ui.role === "controller"
      ? t("button.starting")
      : t("button.connecting");
  } else if (ui.running) {
    els.startLabel.textContent = t("button.running");
  } else {
    els.startLabel.textContent = ui.role === "controller"
      ? t("button.startListening")
      : t("button.connect");
  }
}

function layoutDisabled() {
  return ui.role === "agent" || ui.loading || Boolean(ui.lastStatus?.remoteActive);
}

function screenAspect(screen) {
  const width = Number(screen?.width);
  const height = Number(screen?.height);
  if (!width || !height) return 9 / 16;
  return Math.max(0.25, Math.min(4, height / width));
}

function deviceScreenInfo(placement, peer) {
  return placement?.screen || peer?.screen || null;
}

function buildLayoutItems() {
  const hostInfo = ui.lastStatus?.localScreen || null;
  const host = { x: 0, y: 0, w: 1, h: screenAspect(hostInfo) };
  if (ui.role === "agent") return { host, devices: [] };
  const peerById = new Map((ui.peers || []).map((peer) => [peer.deviceId, peer]));
  const devices = (ui.devices || []).map((placement, index) => {
    const peer = peerById.get(placement.id);
    const screen = deviceScreenInfo(placement, peer);
    const x = Number.isFinite(Number(placement.x)) ? Number(placement.x) : 1 + index;
    const y = Number.isFinite(Number(placement.y)) ? Number(placement.y) : 0;
    return {
      ...placement,
      label: placement.label || peer?.label || t("common.peerControlled"),
      screen,
      online: Boolean(peer?.online),
      active: Boolean(peer?.active) || ui.targetDevice === placement.id,
      rect: { x, y, w: 1, h: screenAspect(screen) },
    };
  });
  for (const peer of ui.peers || []) {
    if (!devices.some((device) => device.id === peer.deviceId)) {
      devices.push({
        id: peer.deviceId,
        label: peer.label || t("common.peerControlled"),
        screen: peer.screen,
        online: Boolean(peer.online),
        active: Boolean(peer.active) || ui.targetDevice === peer.deviceId,
        rect: { x: 1 + devices.length, y: 0, w: 1, h: screenAspect(peer.screen) },
      });
    }
  }
  devices.sort((a, b) => String(a.label).localeCompare(String(b.label)));
  return { host, devices };
}

function createScreenCard({ label, screen, rect, minX, minY, online, active, deviceId, host }) {
  const card = document.createElement("div");
  card.className = `screen ${host ? "host-screen" : "device-screen"}`;
  if (!host) card.classList.add(online ? "is-online" : "is-offline");
  if (active) card.classList.add("is-active");
  card.style.left = `${(rect.x - minX) * WORLD_UNIT}px`;
  card.style.top = `${(rect.y - minY) * WORLD_UNIT}px`;
  card.style.width = `${rect.w * WORLD_UNIT}px`;
  card.style.height = `${Math.max(40, rect.h * WORLD_UNIT)}px`;
  if (!host) {
    card.dataset.deviceId = deviceId;
    card.tabIndex = layoutDisabled() ? -1 : 0;
    card.setAttribute("role", "button");
    card.setAttribute("aria-label", `${label}. ${t("layout.dragHint")}`);
  }
  const content = document.createElement("div");
  content.className = "screen-content";
  const strong = document.createElement("strong");
  strong.textContent = label;
  const size = document.createElement("span");
  size.className = "screen-size";
  size.textContent = screen ? formatScreen(screen) : "—";
  content.append(strong, size);
  card.append(content);
  if (!host) {
    const badge = document.createElement("span");
    badge.className = "device-badge";
    badge.textContent = active
      ? t("status.active")
      : online
        ? t("status.online")
        : t("status.offline");
    card.append(badge);
  }
  return card;
}

function renderLayout() {
  if (!els.layoutWorld) return;
  if (dragState) return;
  const { host, devices } = buildLayoutItems();
  const cards = [host, ...devices.map((device) => device.rect)];
  let minX = Math.min(...cards.map((rect) => rect.x));
  let minY = Math.min(...cards.map((rect) => rect.y));
  let maxX = Math.max(...cards.map((rect) => rect.x + rect.w));
  let maxY = Math.max(...cards.map((rect) => rect.y + rect.h));
  minX -= 0.75;
  minY -= 0.75;
  maxX += 0.75;
  maxY += 0.75;
  const worldW = Math.max(0.5, maxX - minX);
  const worldH = Math.max(0.5, maxY - minY);
  const stageRect = els.layoutStage.getBoundingClientRect();
  const stageW = stageRect.width || 640;
  const stageH = stageRect.height || 320;
  const usableW = Math.max(80, stageW - LAYOUT_PADDING * 2);
  const usableH = Math.max(80, stageH - LAYOUT_PADDING * 2);
  const scale = Math.min(
    usableW / (worldW * WORLD_UNIT),
    usableH / (worldH * WORLD_UNIT),
    1.15,
  );
  const offsetX = (stageW - worldW * WORLD_UNIT * scale) / 2;
  const offsetY = (stageH - worldH * WORLD_UNIT * scale) / 2;
  els.layoutWorld.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
  els.layoutWorld.style.width = `${worldW * WORLD_UNIT}px`;
  els.layoutWorld.style.height = `${worldH * WORLD_UNIT}px`;
  ui.layoutView = { minX, minY, scale };
  els.layoutWorld.innerHTML = "";
  els.layoutWorld.append(
    createScreenCard({
      label: t("common.thisDevice"),
      screen: ui.lastStatus?.localScreen,
      rect: host,
      minX,
      minY,
      online: true,
      active: false,
      host: true,
    }),
  );
  for (const device of devices) {
    els.layoutWorld.append(
      createScreenCard({
        label: device.label,
        screen: device.screen,
        rect: device.rect,
        minX,
        minY,
        online: device.online,
        active: device.active,
        deviceId: device.id,
        host: false,
      }),
    );
  }
  els.layoutEmpty.hidden = devices.length > 0;
  els.layoutStage.classList.toggle("is-disabled", layoutDisabled());
}

function snapDevicePosition(device, allRects) {
  const threshold = LAYOUT_SNAP;
  const rect = { ...device.rect };
  let x = Math.round(rect.x / 0.05) * 0.05;
  let y = Math.round(rect.y / 0.05) * 0.05;
  let bestX = threshold;
  let bestY = threshold;
  for (const other of allRects) {
    for (const edge of [other.x, other.x + other.w, other.x - rect.w, other.x + other.w - rect.w]) {
      const distance = Math.abs(x - edge);
      if (distance < bestX) {
        bestX = distance;
        x = edge;
      }
    }
    for (const edge of [other.y, other.y + other.h, other.y - rect.h, other.y + other.h - rect.h]) {
      const distance = Math.abs(y - edge);
      if (distance < bestY) {
        bestY = distance;
        y = edge;
      }
    }
  }
  return { ...rect, x, y };
}

function rectsOverlap(a, b) {
  const overlapX = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const overlapY = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return overlapX > 0.0005 && overlapY > 0.0005;
}

function onDevicePointerDown(event) {
  if (layoutDisabled() || event.button !== 0 || !els.layoutWorld) return;
  const card = event.target.closest(".device-screen");
  if (!card) return;
  const deviceId = card.dataset.deviceId;
  const placement = (ui.devices || []).find((device) => device.id === deviceId);
  if (!placement) return;
  const view = ui.layoutView;
  if (!view) return;
  dragState = {
    pointerId: event.pointerId,
    deviceId,
    card,
    startX: event.clientX,
    startY: event.clientY,
    origX: Number(placement.x) || 0,
    origY: Number(placement.y) || 0,
    scale: view.scale || 1,
    minX: view.minX || 0,
    minY: view.minY || 0,
    moved: false,
  };
  card.setPointerCapture?.(event.pointerId);
  card.classList.add("is-dragging");
  els.layoutStage.classList.add("is-dragging");
  event.preventDefault();
}

function onDevicePointerMove(event) {
  if (!dragState || event.pointerId !== dragState.pointerId) return;
  const worldScale = WORLD_UNIT * (dragState.scale || 1);
  const dx = (event.clientX - dragState.startX) / worldScale;
  const dy = (event.clientY - dragState.startY) / worldScale;
  if (Math.hypot(dx, dy) > 0.01) dragState.moved = true;
  const x = dragState.origX + dx;
  const y = dragState.origY + dy;
  const placement = (ui.devices || []).find((device) => device.id === dragState.deviceId);
  if (placement) {
    placement.x = x;
    placement.y = y;
  }
  dragState.card.style.left = `${(x - dragState.minX) * WORLD_UNIT}px`;
  dragState.card.style.top = `${(y - dragState.minY) * WORLD_UNIT}px`;
  event.preventDefault();
}

async function onDevicePointerUp(event) {
  if (!dragState || event.pointerId !== dragState.pointerId) return;
  const state = dragState;
  dragState = null;
  state.card.classList.remove("is-dragging");
  els.layoutStage.classList.remove("is-dragging");
  if (state.card.hasPointerCapture?.(event.pointerId)) {
    state.card.releasePointerCapture(event.pointerId);
  }
  if (!state.moved) {
    renderLayout();
    return;
  }

  const { host, devices } = buildLayoutItems();
  const current = devices.find((device) => device.id === state.deviceId);
  if (!current) {
    renderLayout();
    return;
  }
  const others = [host, ...devices.filter((device) => device.id !== state.deviceId).map((device) => device.rect)];
  const snapped = snapDevicePosition(current, others);
  const overlap = others.some((rect) => rectsOverlap(snapped, rect));
  if (overlap) {
    const placement = (ui.devices || []).find((device) => device.id === state.deviceId);
    if (placement) {
      placement.x = state.origX;
      placement.y = state.origY;
    }
    showToast(t("layout.overlap"));
    renderLayout();
    return;
  }

  const placement = (ui.devices || []).find((device) => device.id === state.deviceId);
  if (placement) {
    placement.x = snapped.x;
    placement.y = snapped.y;
  }
  try {
    const config = await withTimeout(invoke("save_layout", {
      devices: (ui.devices || []).map(({ id, label, x, y, screen }) => ({
        id,
        label,
        x,
        y,
        screen: screen || null,
      })),
    }));
    ui.savedConfig = config;
    if (ui.lastState) ui.lastState.config = config;
  } catch (error) {
    if (placement) {
      placement.x = state.origX;
      placement.y = state.origY;
    }
    setError(errorToRaw(error));
  }
  renderLayout();
}

const INTERFACE_STORAGE_KEY = "glidelink.interface";

function updateInterfaces(interfaces, preferredIp) {
  const list = Array.isArray(interfaces) ? interfaces : [];
  const signature = list.map((item) => `${item.name}|${item.ip}`).join(";");
  if (signature !== ui.interfaceSignature) {
    ui.interfaceSignature = signature;
    ui.interfaces = list;
    const previous = els.interfaceSelect.value;
    els.interfaceSelect.innerHTML = "";
    for (const item of list) {
      const option = document.createElement("option");
      option.value = item.ip;
      option.textContent = `${item.name} — ${item.ip}`;
      els.interfaceSelect.append(option);
    }
    let saved = "";
    try {
      saved = localStorage.getItem(INTERFACE_STORAGE_KEY) || "";
    } catch {
      // ignore storage failures
    }
    const candidates = [previous, ui.selectedIp, preferredIp, saved].filter(Boolean);
    const chosen = candidates.find((ip) => list.some((item) => item.ip === ip))
      || list[0]?.ip
      || "";
    els.interfaceSelect.value = chosen;
    ui.selectedIp = chosen;
  } else if (!list.some((item) => item.ip === els.interfaceSelect.value)) {
    els.interfaceSelect.value = list[0]?.ip || "";
    ui.selectedIp = els.interfaceSelect.value;
  } else {
    ui.selectedIp = els.interfaceSelect.value;
  }
  els.selectedIp.textContent = ui.selectedIp || "—";
  updateSettingsControls();
}

async function refreshControllerCode() {
  if (!invoke || ui.role !== "controller" || ui.running || ui.loading) return;
  const ip = ui.selectedIp;
  if (!ip) return;
  if (ui.previewListenIp === ip && getPairingCode().length === PAIRING_CODE_LENGTH) return;
  if (ui.previewListenPromise) return ui.previewListenPromise;
  ui.previewListenPromise = (async () => {
    try {
      const code = await withTimeout(invoke("preview_pairing_code", { localIp: ip }));
      if (
        ui.role === "controller"
        && !ui.running
        && ui.selectedIp === ip
      ) {
        setPairingCode(code);
        ui.lastListenCode = code;
        ui.previewListenIp = ip;
      }
    } catch (error) {
      setError(errorToRaw(error));
    } finally {
      ui.previewListenPromise = null;
    }
  })();
  return ui.previewListenPromise;
}

function setLoading(loading) {
  ui.loading = loading;
  els.startBtn.classList.toggle("loading", loading);
  els.startBtn.setAttribute("aria-busy", String(loading));
  els.startBtn.disabled = loading || ui.running;
  updateSettingsControls();
  updateStartLabel();
}

function setRole(role, syncControls = true) {
  const previousRole = ui.role;
  ui.role = role === "agent" ? "agent" : "controller";
  for (const option of els.roleOptions) {
    const active = option.dataset.role === ui.role;
    option.classList.toggle("active", active);
    option.setAttribute("aria-checked", String(active));
  }
  els.controllerFields.hidden = ui.role !== "controller";
  els.keyboardGroup.hidden = ui.role !== "controller";
  els.networkGroup.hidden = ui.role !== "controller";
  for (const cell of els.codeCells) cell.readOnly = ui.role === "controller";
  if (previousRole !== ui.role) {
    const code = ui.role === "controller"
      ? (ui.lastStatus?.pairingCode || ui.lastListenCode || "")
      : (ui.lastPairingCode || "");
    setPairingCode(code);
  }
  updateInterfaces(ui.interfaces, ui.configLocalIp);
  if (ui.role === "controller" && !ui.running) refreshControllerCode();
  renderLayout();
  updateStartLabel();
  updateSettingsControls();
  if (syncControls && !ui.running) setDisabledControls(false);
}

function setDisabledControls(disabled) {
  for (const input of els.form.querySelectorAll("input")) input.disabled = disabled;
  for (const option of els.roleOptions) option.disabled = disabled;
  els.startBtn.disabled = disabled || ui.loading;
  els.stopBtn.hidden = !disabled;
  updateSettingsControls();
  updateStartLabel();
}

/// Settings that only affect the next session are locked while one runs.
/// Language is safe to change at any time.
function updateSettingsControls() {
  const busy = ui.running || ui.loading;
  els.adaptModifiers.disabled = busy;
  els.interfaceSelect.disabled = busy;
}

function syncLanguage(configLanguage) {
  if (ui.languageSynced) return;
  ui.languageSynced = true;
  const fromSettings = configLanguage === "zh" || configLanguage === "en"
    ? configLanguage
    : "";
  const cached = i18n.getStoredLocale();
  ui.languageSyncGuard = true;
  try {
    if (fromSettings) {
      ui.language = fromSettings;
      i18n.setLocale(fromSettings);
    } else {
      ui.language = i18n.getLocale();
      // Import an explicit choice made in a pre-settings build once.
      if (cached) saveSettings({ language: cached });
    }
  } finally {
    ui.languageSyncGuard = false;
  }
  els.languageSelect.value = ui.language;
}

function revertSettingsFromSaved() {
  const config = ui.savedConfig;
  if (!config) return;
  els.adaptModifiers.checked = Boolean(config.adaptModifiers);
  ui.adaptModifiers = Boolean(config.adaptModifiers);
  if (config.logLevel) els.logLevel.value = config.logLevel;
  ui.configLocalIp = config.localIp || "";
  const preferred = config.localIp || ui.selectedIp;
  updateInterfaces(ui.interfaces, preferred);
  if (config.language && config.language !== ui.language) {
    ui.language = config.language;
    i18n.setLocale(config.language);
  }
  els.languageSelect.value = ui.language;
}

async function saveSettings(patch) {
  if (!invoke) return null;
  const promise = withTimeout(invoke("save_settings", { patch }))
    .then((config) => {
      ui.savedConfig = config;
      ui.adaptModifiers = Boolean(config.adaptModifiers);
      els.adaptModifiers.checked = ui.adaptModifiers;
      if (config.logLevel) els.logLevel.value = config.logLevel;
      ui.configLocalIp = config.localIp || "";
      if (config.language) {
        ui.language = config.language;
        els.languageSelect.value = config.language;
      }
      return config;
    })
    .catch((error) => {
      revertSettingsFromSaved();
      setError(errorToRaw(error));
      return null;
    })
    .finally(() => {
      if (ui.settingsSavePromise === promise) ui.settingsSavePromise = null;
    });
  ui.settingsSavePromise = promise;
  return promise;
}

function withTimeout(promise, ms = COMMAND_TIMEOUT_MS) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("COMMAND_TIMEOUT")), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

function applyState(state) {
  if (!state) return;
  ui.lastState = state;
  const { config, status: incomingStatus, platform, permissionsOk } = state;
  const wasRunning = ui.running;
  ui.running = Boolean(incomingStatus?.running);
  ui.lastStatus = incomingStatus || null;
  if (config) {
    ui.savedConfig = config;
    ui.lastPairingCode = config.lastPairingCode || ui.lastPairingCode || "";
    ui.lastListenCode = config.lastInvite || ui.lastListenCode || "";
    if (!dragState) {
      ui.devices = Array.isArray(config.layout) ? config.layout : [];
    }
  }
  ui.peers = Array.isArray(incomingStatus?.devices) ? incomingStatus.devices : [];
  ui.targetDevice = incomingStatus?.targetDevice || null;

  if (config && !ui.initialized) {
    ui.savedConfig = config;
    ui.configLocalIp = config.localIp || "";
    ui.interfaces = Array.isArray(state.localInterfaces) ? state.localInterfaces : [];
    ui.adaptModifiers = Boolean(config.adaptModifiers);
    els.adaptModifiers.checked = ui.adaptModifiers;
    if (config.logLevel) els.logLevel.value = config.logLevel;
    setRole(config.role, false);
    setPairingCode(
      config.role === "controller"
        ? (incomingStatus?.pairingCode || config.lastInvite || "")
        : (config.lastPairingCode || ""),
    );
    updateInterfaces(ui.interfaces, ui.configLocalIp);
    if (ui.role === "controller" && !ui.running) refreshControllerCode();
    setDisabledControls(ui.running);
    ui.initialized = true;
    syncLanguage(config.language);
    // Persist the interface the UI had to fall back to (fresh install, or a
    // saved interface that no longer exists).
    if (ui.role === "controller" && !config.localIp && ui.selectedIp) {
      saveSettings({ localIp: ui.selectedIp });
    }
  } else if (config && !ui.running && wasRunning) {
    setDisabledControls(false);
    if (ui.role === "controller") {
      setPairingCode(ui.lastListenCode || "");
      ui.previewListenIp = ui.selectedIp;
    } else {
      setPairingCode(ui.lastPairingCode || "");
    }
  }

  if (Array.isArray(state.localInterfaces)) {
    ui.interfaces = state.localInterfaces;
    updateInterfaces(ui.interfaces, ui.configLocalIp);
  }

  if (ui.role === "controller" && incomingStatus?.running && incomingStatus.pairingCode) {
    ui.lastListenCode = incomingStatus.pairingCode;
    ui.previewListenIp = ui.selectedIp;
    setPairingCode(incomingStatus.pairingCode);
  } else if (
    ui.role === "controller"
    && !ui.running
    && getPairingCode().length !== PAIRING_CODE_LENGTH
  ) {
    refreshControllerCode();
  }

  if (ui.running) setDisabledControls(true);

  const permissionBlocked = platform === "macos" && permissionsOk === false;
  els.permissionCard.hidden = !permissionBlocked;

  const status = ui.lastStatus || {};
  const connected = Boolean(status.connected);
  const active = Boolean(status.remoteActive);
  const localScreen = status.localScreen;
  const latency = status.latencyMs == null ? null : Number(status.latencyMs);
  const statusError = status.lastErrorCode
    ? `${status.lastErrorCode}${status.lastErrorDetail ? `|${status.lastErrorDetail}` : ""}`
    : null;
  const errorRaw = ui.startError || statusError || (permissionBlocked && ui.running ? "errors.PERMISSIONS_BLOCKED" : null);

  els.localScreenText.textContent = formatScreen(localScreen);
  els.latencyValue.textContent = latency == null ? "—" : `${latency.toFixed(1)} ms`;
  els.latencyDot.className = `latency-dot${connected && latency != null ? (latency < 10 ? " good" : " warn") : ""}`;
  const peerCount = Array.isArray(status.devices) ? status.devices.length : 0;
  els.deviceCount.textContent = peerCount === 0 ? "—" : String(peerCount);
  els.peerText.textContent = status.peer
    || (peerCount > 0 ? `${peerCount} ${t("status.devices")}` : t("common.notConnected"));
  els.layoutStage.classList.toggle("remote-active", active);
  els.layoutStage.classList.toggle("is-connected", connected);

  const activeLabel = ui.role === "agent" ? t("status.controlled") : t("status.controlling");
  els.sessionState.textContent = sessionStateLabel();
  renderLayout();

  let pillClass = "status-idle";
  let text = t("status.notRunning");
  if (ui.running) {
    if (errorRaw) {
      pillClass = "status-error";
      text = t("status.error");
    } else if (active) {
      pillClass = "status-active";
      text = activeLabel;
    } else if (connected) {
      pillClass = "status-connected";
      text = t("status.connected");
    } else {
      pillClass = "status-waiting";
      text = ui.role === "controller" ? t("status.listening") : t("status.waiting");
    }
  } else if (errorRaw) {
    pillClass = "status-error";
    text = t("status.startFailed");
  }

  els.statusPill.className = `status-pill ${pillClass}`;
  els.statusText.textContent = text;
  els.startBtn.disabled = ui.running || ui.loading;
  els.stopBtn.hidden = !ui.running;
  updateStartLabel();

  if (ui.running && connected) setError(null);
  if (errorRaw) {
    els.errorText.textContent = translateError(errorRaw);
    els.errorBanner.hidden = false;
  } else if (!ui.startError) {
    els.errorBanner.hidden = true;
  }

  updateAboutStats(status);
}

async function refreshLogs() {
  if (!invoke || ui.page !== "logs") return;
  try {
    const entries = await withTimeout(invoke("get_logs"));
    const panel = els.logPanel;
    const atBottom = panel.scrollTop + panel.clientHeight >= panel.scrollHeight - 8;
    const lines = (Array.isArray(entries) ? entries : []).map((entry) => {
      const seconds = (Number(entry.elapsedMs || 0) / 1000).toFixed(3).padStart(9, " ");
      const level = String(entry.level || "").padEnd(5, " ");
      return `[${seconds}s] ${level} ${entry.target || ""}: ${entry.message || ""}`;
    });
    panel.textContent = lines.length ? lines.join("\n") : t("logs.empty");
    els.logCount.textContent = lines.length ? String(lines.length) : "";
    if (atBottom) panel.scrollTop = panel.scrollHeight;
  } catch (error) {
    els.logPanel.textContent = errorToRaw(error);
  }
}

async function refresh() {
  if (!invoke) return;
  try {
    const state = await withTimeout(invoke("get_state"));
    applyState(state);
  } catch (error) {
    setError(errorToRaw(error));
  }
}

async function startSession(event) {
  event.preventDefault();
  if (!invoke) {
    setError("errors.UNKNOWN");
    return;
  }
  if (ui.page !== "session") setPage("session");

  if (ui.role === "controller" && !ui.selectedIp) {
    setError("validation.interface");
    return;
  }
  const pairingCode = ui.role === "agent" ? getPairingCode() : "";
  if (ui.role === "agent" && pairingCode.length !== PAIRING_CODE_LENGTH) {
    setError("validation.pairingCode");
    els.codeCells[0]?.focus();
    return;
  }

  setError(null);
  setLoading(true);
  try {
    if (ui.settingsSavePromise) await ui.settingsSavePromise;
    await withTimeout(invoke("start_session", {
      config: {
        role: ui.role,
        localIp: ui.role === "controller" ? ui.selectedIp : "",
        adaptModifiers: ui.role === "controller" && ui.adaptModifiers,
        language: ui.language || "",
      },
      pairingCode,
    }));
    showToast(ui.role === "controller" ? t("toast.startedController") : t("toast.startedAgent"));
    await refresh();
  } catch (error) {
    setError(errorToRaw(error));
  } finally {
    setLoading(false);
    await refresh();
  }
}

async function copyPasscode() {
  const code = getPairingCode();
  if (!code) {
    showToast(t("toast.passcodeEmpty"));
    return;
  }
  let copied = false;
  try {
    await navigator.clipboard.writeText(code);
    copied = true;
  } catch {
    const temporary = document.createElement("textarea");
    temporary.value = code;
    temporary.style.position = "fixed";
    temporary.style.opacity = "0";
    document.body.appendChild(temporary);
    temporary.select();
    try {
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    } finally {
      temporary.remove();
    }
  }
  showToast(copied ? t("toast.passcodeCopied") : t("toast.passcodeCopyFailed"));
}

async function stopSession() {
  if (!invoke) return;
  els.stopBtn.disabled = true;
  try {
    await withTimeout(invoke("stop_session"));
    setError(null);
    showToast(t("toast.stopped"));
  } catch (error) {
    setError(errorToRaw(error));
  } finally {
    els.stopBtn.disabled = false;
    await refresh();
  }
}

function initEvents() {
  for (const item of els.navItems) {
    item.addEventListener("click", () => setPage(item.dataset.pageTarget));
  }

  for (const option of els.roleOptions) {
    option.addEventListener("click", () => {
      if (ui.running) return;
      setRole(option.dataset.role);
    });
  }

  window.addEventListener("resize", () => renderLayout());
  els.layoutWorld.addEventListener("pointerdown", onDevicePointerDown);
  els.layoutWorld.addEventListener("pointermove", onDevicePointerMove);
  els.layoutWorld.addEventListener("pointerup", onDevicePointerUp);
  els.layoutWorld.addEventListener("pointercancel", onDevicePointerUp);
  els.layoutWorld.addEventListener("lostpointercapture", () => {
    if (!dragState) return;
    dragState = null;
    els.layoutStage.classList.remove("is-dragging");
    renderLayout();
  });

  els.copyPasscode.addEventListener("click", copyPasscode);
  els.logLevel.addEventListener("change", () => {
    saveSettings({ logLevel: els.logLevel.value });
    refreshLogs();
  });
  els.copyLogs.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(els.logPanel.textContent || "");
      showToast(t("logs.copy"));
    } catch (error) {
      setError(errorToRaw(error));
    }
  });
  els.clearLogs.addEventListener("click", async () => {
    try {
      await invoke("clear_logs");
      refreshLogs();
    } catch (error) {
      setError(errorToRaw(error));
    }
  });

  els.codeCells.forEach((cell, index) => {
    cell.addEventListener("input", () => {
      if (ui.role === "controller") return;
      const normalized = normalizeCodePart(cell.value, 4);
      if (cell.value !== normalized) cell.value = normalized;
      if (normalized.length === 4 && index < els.codeCells.length - 1) {
        els.codeCells[index + 1].focus();
      }
    });
    cell.addEventListener("keydown", (event) => {
      if (ui.role === "controller") return;
      if (event.key === "Backspace" && !cell.value && index > 0) {
        els.codeCells[index - 1].focus();
        event.preventDefault();
      } else if (event.key === "ArrowLeft" && index > 0 && cell.selectionStart === 0) {
        els.codeCells[index - 1].focus();
        event.preventDefault();
      } else if (
        event.key === "ArrowRight"
        && index < els.codeCells.length - 1
        && cell.selectionStart === cell.value.length
      ) {
        els.codeCells[index + 1].focus();
        event.preventDefault();
      }
    });
    cell.addEventListener("paste", (event) => {
      if (ui.role === "controller") return;
      event.preventDefault();
      const text = normalizeCodePart(event.clipboardData?.getData("text") || "");
      if (!text) return;
      setPairingCode(text);
      const filled = Math.min(Math.ceil(text.length / 4), els.codeCells.length);
      els.codeCells[Math.max(0, filled - 1)]?.focus();
    });
  });

  els.interfaceSelect.addEventListener("change", () => {
    if (ui.running) return;
    ui.selectedIp = els.interfaceSelect.value;
    els.selectedIp.textContent = ui.selectedIp || "—";
    ui.lastListenCode = "";
    ui.previewListenIp = "";
    setPairingCode("");
    saveSettings({ localIp: ui.selectedIp });
    refreshControllerCode();
  });

  els.adaptModifiers.addEventListener("change", () => {
    if (ui.running) return;
    ui.adaptModifiers = els.adaptModifiers.checked;
    saveSettings({ adaptModifiers: ui.adaptModifiers });
  });

  els.openNetworkSettings.addEventListener("click", () => setPage("settings"));
  els.form.addEventListener("submit", startSession);
  els.stopBtn.addEventListener("click", stopSession);
  els.permissionBtn.addEventListener("click", async () => {
    try {
      await invoke("open_permissions");
    } catch (error) {
      setError(errorToRaw(error));
    }
  });

  els.reopenBtn.addEventListener("click", async () => {
    try {
      await invoke("reopen_app");
    } catch (error) {
      setError(errorToRaw(error));
    }
  });

  els.languageSelect.addEventListener("change", () => {
    const next = els.languageSelect.value;
    ui.language = next;
    i18n.setLocale(next);
    saveSettings({ language: next });
  });

  document.addEventListener("glidelink:locale", () => {
    if (ui.languageSyncGuard) return;
    updatePageChrome();
    updateStartLabel();
    renderLayout();
    setRole(ui.role, false);
    if (ui.lastState) applyState(ui.lastState);
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      if (ui.page !== "session") setPage("session");
      if (ui.running || ui.loading) return;
      els.form.requestSubmit();
    }
  });
}

async function boot() {
  initEvents();
  els.languageSelect.value = i18n.getLocale();
  setRole("controller", false);
  setPage("session");
  if (!invoke) {
    setError("errors.UNKNOWN");
    return;
  }
  try {
    const state = await withTimeout(invoke("get_state"));
    applyState(state);
  } catch (error) {
    setError(errorToRaw(error));
  }
  setInterval(refresh, 500);
  setInterval(() => {
    if (ui.page === "logs") refreshLogs();
  }, 1000);
}

boot();
