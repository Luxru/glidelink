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
  agentFields: document.getElementById("agentFields"),
  peerAddr: document.getElementById("peerAddr"),
  listenPort: document.getElementById("listenPort"),
  passcode: document.getElementById("passcode"),
  startBtn: document.getElementById("startBtn"),
  startLabel: document.getElementById("startLabel"),
  stopBtn: document.getElementById("stopBtn"),
  statusPill: document.getElementById("statusPill"),
  statusText: document.getElementById("statusText"),
  errorBanner: document.getElementById("errorBanner"),
  errorText: document.getElementById("errorText"),
  layoutStage: document.getElementById("layoutStage"),
  hostScreen: document.getElementById("hostScreen"),
  hostScreenSize: document.getElementById("hostScreenSize"),
  remoteScreenSize: document.getElementById("remoteScreenSize"),
  hostScreenLabel: document.getElementById("hostScreenLabel"),
  remoteScreenLabel: document.getElementById("remoteScreenLabel"),
  remoteBadge: document.getElementById("remoteBadge"),
  linkChipText: document.getElementById("linkChipText"),
  latencyDot: document.getElementById("latencyDot"),
  latencyValue: document.getElementById("latencyValue"),
  localScreenText: document.getElementById("localScreenText"),
  peerText: document.getElementById("peerText"),
  permissionCard: document.getElementById("permissionCard"),
  permissionBtn: document.getElementById("permissionBtn"),
  togglePasscode: document.getElementById("togglePasscode"),
  copyPasscode: document.getElementById("copyPasscode"),
  generatePasscode: document.getElementById("generatePasscode"),
  sessionState: document.getElementById("sessionState"),
  layoutSummary: document.getElementById("layoutSummary"),
  editLayout: document.getElementById("editLayout"),
  remoteScreen: document.getElementById("remoteScreen"),
  layoutDragHint: document.getElementById("layoutDragHint"),
  aboutRole: document.getElementById("aboutRole"),
  aboutState: document.getElementById("aboutState"),
  aboutDuration: document.getElementById("aboutDuration"),
  aboutPeer: document.getElementById("aboutPeer"),
  aboutLinkState: document.getElementById("aboutLinkState"),
  aboutRtt: document.getElementById("aboutRtt"),
  aboutLocalScreen: document.getElementById("aboutLocalScreen"),
  aboutRemoteScreen: document.getElementById("aboutRemoteScreen"),
  aboutPacketsSent: document.getElementById("aboutPacketsSent"),
  aboutPacketsReceived: document.getElementById("aboutPacketsReceived"),
  aboutBytesSent: document.getElementById("aboutBytesSent"),
  aboutBytesReceived: document.getElementById("aboutBytesReceived"),
  toast: document.getElementById("toast"),
};

const ui = {
  page: "session",
  role: "controller",
  edge: "right",
  offset: 0,
  running: false,
  loading: false,
  initialized: false,
  lastStatus: null,
  lastState: null,
  startError: null,
};

const SCREEN_W = 250;
const SCREEN_H = 150;
const SCREEN_GAP = 12;
const MAX_OFFSET = 0.65;
const DRAG_EDGES = ["left", "right", "top", "bottom"];

let dragState = null;
let lastDrag = null;

const PAGES = {
  session: "page.session.title",
  layout: "page.layout.title",
  about: "page.about.title",
};

const EDGE_SUMMARIES = {
  left: "layout.summaryLeft",
  right: "layout.summaryRight",
  top: "layout.summaryTop",
  bottom: "layout.summaryBottom",
};

const PASSCODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const COMMAND_TIMEOUT_MS = 12000;

let toastTimer;

function generatePasscode(length = 12) {
  const bytes = new Uint8Array(length);
  crypto.getRandomValues(bytes);
  let code = "";
  for (let i = 0; i < length; i += 1) {
    code += PASSCODE_ALPHABET[bytes[i] % PASSCODE_ALPHABET.length];
  }
  return code;
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
  els.aboutLinkState.textContent = connected
    ? t("status.connected")
    : running
      ? t("status.waiting")
      : t("status.notRunning");
  els.aboutRtt.textContent = latency == null ? "—" : `${latency.toFixed(1)} ms`;
  els.aboutLocalScreen.textContent = formatScreen(status?.localScreen);
  els.aboutRemoteScreen.textContent = formatScreen(status?.remoteScreen);
  els.aboutPacketsSent.textContent = formatCount(status?.packetsSent);
  els.aboutPacketsReceived.textContent = formatCount(status?.packetsReceived);
  els.aboutBytesSent.textContent = formatBytes(status?.bytesSent);
  els.aboutBytesReceived.textContent = formatBytes(status?.bytesReceived);
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
}

function updateStartLabel() {
  if (ui.loading) {
    els.startLabel.textContent = ui.role === "controller"
      ? t("button.connecting")
      : t("button.listening");
  } else if (ui.running) {
    els.startLabel.textContent = t("button.running");
  } else {
    els.startLabel.textContent = ui.role === "controller"
      ? t("button.startControl")
      : t("button.startListening");
  }
}

function layoutDisabled() {
  return ui.running || ui.loading || ui.role === "agent";
}

function peerPosition() {
  const edge = ui.role === "agent" ? "right" : ui.edge;
  const offset = ui.role === "agent" ? 0 : ui.offset;
  let hostDx = 0;
  let hostDy = 0;
  let peerDx = 0;
  let peerDy = 0;
  if (edge === "left" || edge === "right") {
    const half = (SCREEN_W + SCREEN_GAP) / 2;
    hostDx = edge === "right" ? -half : half;
    peerDx = -hostDx;
    peerDy = offset * SCREEN_H;
  } else {
    const half = (SCREEN_H + SCREEN_GAP) / 2;
    hostDy = edge === "bottom" ? -half : half;
    peerDy = -hostDy;
    peerDx = offset * SCREEN_W;
  }
  return { hostDx, hostDy, peerDx, peerDy };
}

function renderLayout() {
  const { hostDx, hostDy, peerDx, peerDy } = peerPosition();
  els.hostScreen.style.transform =
    `translate(calc(-50% + ${hostDx}px), calc(-50% + ${hostDy}px))`;
  if (!dragState) {
    els.remoteScreen.style.transform =
      `translate(calc(-50% + ${peerDx}px), calc(-50% + ${peerDy}px))`;
  }
  const disabled = layoutDisabled();
  els.layoutStage.dataset.edge = ui.role === "agent" ? "right" : ui.edge;
  els.layoutStage.classList.toggle("is-disabled", disabled);
  els.remoteScreen.tabIndex = disabled ? -1 : 0;
  els.remoteScreen.setAttribute("aria-disabled", String(disabled));
}

function updateLayoutSummary() {
  els.layoutSummary.textContent = ui.role === "agent"
    ? t("layout.summaryAgent")
    : t(EDGE_SUMMARIES[ui.edge]);
  els.layoutDragHint.hidden = layoutDisabled();
  renderLayout();
}

function setEdge(edge) {
  ui.edge = DRAG_EDGES.includes(edge) ? edge : "right";
  updateLayoutSummary();
}

function setOffset(offset) {
  const value = Number(offset);
  ui.offset = Number.isFinite(value)
    ? Math.max(-MAX_OFFSET, Math.min(MAX_OFFSET, value))
    : 0;
  renderLayout();
}

function dominantEdge(dx, dy) {
  if (Math.abs(dx) >= Math.abs(dy)) return dx >= 0 ? "right" : "left";
  return dy >= 0 ? "bottom" : "top";
}

function onPeerPointerDown(event) {
  if (layoutDisabled() || event.button !== 0) return;
  const { peerDx, peerDy } = peerPosition();
  dragState = {
    pointerId: event.pointerId,
    startX: event.clientX,
    startY: event.clientY,
    peerDx,
    peerDy,
  };
  lastDrag = null;
  els.layoutStage.classList.add("is-dragging");
  els.remoteScreen.setPointerCapture?.(event.pointerId);
  els.remoteScreen.focus({ preventScroll: true });
  event.preventDefault();
}

function onPeerPointerMove(event) {
  if (!dragState || event.pointerId !== dragState.pointerId) return;
  const stageRect = els.layoutStage.getBoundingClientRect();
  let dx = dragState.peerDx + (event.clientX - dragState.startX);
  let dy = dragState.peerDy + (event.clientY - dragState.startY);
  const limitX = Math.max(0, stageRect.width / 2 - SCREEN_W / 2 - 6);
  const limitY = Math.max(0, stageRect.height / 2 - SCREEN_H / 2 - 6);
  dx = Math.max(-limitX, Math.min(limitX, dx));
  dy = Math.max(-limitY, Math.min(limitY, dy));
  const pointerDx = event.clientX - (stageRect.left + stageRect.width / 2);
  const pointerDy = event.clientY - (stageRect.top + stageRect.height / 2);
  lastDrag = { dx, dy, pointerDx, pointerDy };
  els.remoteScreen.style.transform =
    `translate(calc(-50% + ${dx}px), calc(-50% + ${dy}px))`;
  els.layoutStage.dataset.edge = dominantEdge(pointerDx, pointerDy);
}

function onPeerPointerUp(event) {
  if (!dragState || event.pointerId !== dragState.pointerId) return;
  const moved = lastDrag
    ? Math.hypot(lastDrag.dx - dragState.peerDx, lastDrag.dy - dragState.peerDy)
    : 0;
  if (moved >= 6 && lastDrag) {
    const edge = dominantEdge(lastDrag.pointerDx, lastDrag.pointerDy);
    const rawOffset = edge === "left" || edge === "right"
      ? lastDrag.dy / SCREEN_H
      : lastDrag.dx / SCREEN_W;
    ui.edge = edge;
    ui.offset = Math.max(-MAX_OFFSET, Math.min(MAX_OFFSET, rawOffset));
  }
  dragState = null;
  lastDrag = null;
  els.layoutStage.classList.remove("is-dragging");
  if (els.remoteScreen.hasPointerCapture?.(event.pointerId)) {
    els.remoteScreen.releasePointerCapture(event.pointerId);
  }
  updateLayoutSummary();
  renderLayout();
}

function onPeerKeyDown(event) {
  if (layoutDisabled()) return;
  const step = 0.05;
  if (event.key === "0") {
    setOffset(0);
    event.preventDefault();
    return;
  }
  const shift = event.shiftKey;
  const horizontal = ui.edge === "left" || ui.edge === "right";
  if (shift && horizontal && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
    setOffset(ui.offset + (event.key === "ArrowDown" ? step : -step));
    event.preventDefault();
    return;
  }
  if (shift && !horizontal && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
    setOffset(ui.offset + (event.key === "ArrowRight" ? step : -step));
    event.preventDefault();
    return;
  }
  const edges = {
    ArrowLeft: "left",
    ArrowRight: "right",
    ArrowUp: "top",
    ArrowDown: "bottom",
  };
  if (edges[event.key]) {
    setEdge(edges[event.key]);
    event.preventDefault();
  }
}

function setLoading(loading) {
  ui.loading = loading;
  els.startBtn.classList.toggle("loading", loading);
  els.startBtn.setAttribute("aria-busy", String(loading));
  els.startBtn.disabled = loading || ui.running;
  updateLayoutSummary();
  updateStartLabel();
}

function setRole(role, syncControls = true) {
  ui.role = role === "agent" ? "agent" : "controller";
  for (const option of els.roleOptions) {
    const active = option.dataset.role === ui.role;
    option.classList.toggle("active", active);
    option.setAttribute("aria-checked", String(active));
  }
  els.controllerFields.hidden = ui.role !== "controller";
  els.agentFields.hidden = ui.role !== "agent";
  els.hostScreenLabel.textContent = t("common.thisDevice");
  els.remoteScreenLabel.textContent = ui.role === "controller"
    ? t("common.peerControlled")
    : t("common.peerController");
  updateLayoutSummary();
  updateStartLabel();
  if (syncControls && !ui.running) setDisabledControls(false);
}

function setDisabledControls(disabled) {
  for (const input of els.form.querySelectorAll("input")) input.disabled = disabled;
  for (const option of els.roleOptions) option.disabled = disabled;
  els.generatePasscode.disabled = disabled;
  els.togglePasscode.disabled = disabled;
  els.editLayout.disabled = disabled;
  els.startBtn.disabled = disabled || ui.loading;
  els.stopBtn.hidden = !disabled;
  updateLayoutSummary();
  updateStartLabel();
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

  if (config && !ui.initialized) {
    els.peerAddr.value = config.peerAddr || "";
    els.listenPort.value = config.listenPort || 45890;
    els.passcode.value = config.passcode || "";
    setRole(config.role, false);
    setEdge(config.edge);
    setOffset(config.peerOffset ?? 0);
    setDisabledControls(ui.running);
    ui.initialized = true;
  } else if (config && !ui.running && wasRunning) {
    setDisabledControls(false);
  }

  if (ui.running) setDisabledControls(true);

  const permissionBlocked = platform === "macos" && permissionsOk === false;
  els.permissionCard.hidden = !permissionBlocked;

  const status = ui.lastStatus || {};
  const connected = Boolean(status.connected);
  const active = Boolean(status.remoteActive);
  const localScreen = status.localScreen;
  const remoteScreen = status.remoteScreen;
  const latency = status.latencyMs == null ? null : Number(status.latencyMs);
  const statusError = status.lastErrorCode
    ? `${status.lastErrorCode}${status.lastErrorDetail ? `|${status.lastErrorDetail}` : ""}`
    : null;
  const errorRaw = ui.startError || statusError || (permissionBlocked && ui.running ? "errors.PERMISSIONS_BLOCKED" : null);

  els.hostScreenSize.textContent = formatScreen(localScreen);
  els.remoteScreenSize.textContent = remoteScreen
    ? formatScreen(remoteScreen)
    : (ui.role === "controller" ? t("common.waitingConnection") : t("common.waitingController"));
  els.localScreenText.textContent = formatScreen(localScreen);
  els.latencyValue.textContent = latency == null ? "—" : `${latency.toFixed(1)} ms`;
  els.latencyDot.className = `latency-dot${connected && latency != null ? (latency < 10 ? " good" : " warn") : ""}`;
  els.peerText.textContent = status.peer || t("common.notConnected");
  els.remoteBadge.hidden = !active;
  els.layoutStage.classList.toggle("remote-active", active);
  els.layoutStage.classList.toggle("is-connected", connected);

  const activeLabel = ui.role === "agent" ? t("status.controlled") : t("status.controlling");
  els.sessionState.textContent = !ui.running
    ? t("status.notStarted")
    : active
      ? activeLabel
      : connected
        ? t("status.connected")
        : t("status.waitingPeer");

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
      text = t("status.waiting");
    }
  } else if (errorRaw) {
    pillClass = "status-error";
    text = t("status.startFailed");
  }

  els.statusPill.className = `status-pill ${pillClass}`;
  els.statusText.textContent = text;
  els.linkChipText.textContent = active
    ? (ui.role === "agent" ? t("layout.previewReceiving") : t("layout.previewForwarding"))
    : connected
      ? t("layout.previewReady")
      : ui.running
        ? t("status.waitingPeer")
        : t("layout.previewWaiting");
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

  const passcode = els.passcode.value.trim();
  const peerAddr = els.peerAddr.value.trim();
  const listenPort = Number(els.listenPort.value);

  if (passcode.length < 8) {
    setError("validation.passcodeLength");
    els.passcode.focus();
    return;
  }
  if (ui.role === "controller" && !/^[^\s:]+:\d{1,5}$/.test(peerAddr)) {
    setError("validation.peerAddress");
    els.peerAddr.focus();
    return;
  }
  if (ui.role === "agent" && (!Number.isInteger(listenPort) || listenPort < 1024 || listenPort > 65535)) {
    setError("validation.port");
    els.listenPort.focus();
    return;
  }

  setError(null);
  setLoading(true);
  try {
    await withTimeout(invoke("start_session", {
      config: {
        role: ui.role,
        peerAddr,
        listenPort,
        passcode,
        edge: ui.edge,
        peerOffset: ui.offset,
      },
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
  const code = els.passcode.value.trim();
  if (!code) {
    showToast(t("toast.passcodeEmpty"));
    return;
  }
  let copied = false;
  try {
    await navigator.clipboard.writeText(code);
    copied = true;
  } catch {
    const previousType = els.passcode.type;
    try {
      els.passcode.type = "text";
      els.passcode.focus();
      els.passcode.select();
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    } finally {
      els.passcode.type = previousType;
      window.getSelection()?.removeAllRanges();
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

  els.remoteScreen.addEventListener("pointerdown", onPeerPointerDown);
  els.remoteScreen.addEventListener("pointermove", onPeerPointerMove);
  els.remoteScreen.addEventListener("pointerup", onPeerPointerUp);
  els.remoteScreen.addEventListener("pointercancel", onPeerPointerUp);
  els.remoteScreen.addEventListener("lostpointercapture", () => {
    if (!dragState) return;
    dragState = null;
    lastDrag = null;
    els.layoutStage.classList.remove("is-dragging");
    updateLayoutSummary();
  });
  els.remoteScreen.addEventListener("keydown", onPeerKeyDown);

  els.togglePasscode.addEventListener("click", () => {
    const visible = els.passcode.type === "password";
    els.passcode.type = visible ? "text" : "password";
    els.togglePasscode.setAttribute("aria-pressed", String(visible));
  });

  els.copyPasscode.addEventListener("click", copyPasscode);

  els.generatePasscode.addEventListener("click", () => {
    els.passcode.value = generatePasscode();
    els.passcode.type = "text";
    els.togglePasscode.setAttribute("aria-pressed", "true");
    showToast(t("toast.passcodeGenerated"));
  });

  els.editLayout.addEventListener("click", () => setPage("layout"));
  els.form.addEventListener("submit", startSession);
  els.stopBtn.addEventListener("click", stopSession);
  els.permissionBtn.addEventListener("click", async () => {
    try {
      await invoke("open_permissions");
    } catch (error) {
      setError(errorToRaw(error));
    }
  });

  els.languageSelect.addEventListener("change", () => {
    i18n.setLocale(els.languageSelect.value);
  });

  document.addEventListener("glidelink:locale", () => {
    updatePageChrome();
    updateStartLabel();
    updateLayoutSummary();
    setEdge(ui.edge);
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
  setEdge("right");
  setPage("session");
  if (!invoke) {
    setError("errors.UNKNOWN");
    return;
  }
  try {
    const state = await withTimeout(invoke("get_state"));
    if (state?.config && !state.config.passcode) {
      state.config.passcode = generatePasscode();
    }
    applyState(state);
  } catch (error) {
    setError(errorToRaw(error));
  }
  setInterval(refresh, 500);
}

boot();
