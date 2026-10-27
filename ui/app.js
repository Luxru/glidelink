const invoke = window.__TAURI__?.core?.invoke;
const i18n = window.glidelinkI18n;
const t = (key, vars) => i18n.t(key, vars);

const els = {
  form: document.getElementById("configForm"),
  pageScroll: document.querySelector(".page-scroll"),
  pageTitle: document.getElementById("pageTitle"),
  pageSubtitle: document.getElementById("pageSubtitle"),
  navItems: [...document.querySelectorAll(".nav-item")],
  languageSelect: document.getElementById("languageSelect"),
  roleOptions: [...document.querySelectorAll(".role-option")],
  edgeOptions: [...document.querySelectorAll(".edge-option")],
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
  layoutEdgeRow: document.getElementById("layoutEdgeRow"),
  layoutEdgeSeparator: document.getElementById("layoutEdgeSeparator"),
  toast: document.getElementById("toast"),
};

const ui = {
  page: "session",
  role: "controller",
  edge: "right",
  running: false,
  loading: false,
  initialized: false,
  lastStatus: null,
  lastState: null,
  startError: null,
};

const PAGES = {
  session: {
    title: "page.session.title",
    subtitle: "page.session.subtitle.controller",
    subtitleAgent: "page.session.subtitle.agent",
  },
  layout: { title: "page.layout.title", subtitle: "page.layout.subtitle" },
  security: { title: "page.security.title", subtitle: "page.security.subtitle" },
  about: { title: "page.about.title", subtitle: "page.about.subtitle" },
};

const EDGE_LABELS = {
  left: "layout.edgeLeft",
  right: "layout.edgeRight",
  top: "layout.edgeTop",
  bottom: "layout.edgeBottom",
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
  const page = PAGES[ui.page] || PAGES.session;
  els.pageTitle.textContent = t(page.title);
  els.pageSubtitle.textContent =
    ui.page === "session" && ui.role === "agent"
      ? t(page.subtitleAgent)
      : t(page.subtitle);
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

function updateLayoutSummary() {
  if (ui.role === "agent") {
    els.layoutSummary.textContent = t("layout.summaryAgent");
    els.layoutEdgeRow.hidden = true;
    els.layoutEdgeSeparator.hidden = true;
    els.layoutStage.dataset.edge = "right";
    return;
  }
  els.layoutSummary.textContent = t(EDGE_SUMMARIES[ui.edge]);
  els.layoutEdgeRow.hidden = false;
  els.layoutEdgeSeparator.hidden = false;
  els.layoutStage.dataset.edge = ui.edge;
}

function setLoading(loading) {
  ui.loading = loading;
  els.startBtn.classList.toggle("loading", loading);
  els.startBtn.setAttribute("aria-busy", String(loading));
  els.startBtn.disabled = loading || ui.running;
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
  if (ui.page === "session") updatePageChrome();
  if (syncControls && !ui.running) setDisabledControls(false);
}

function setEdge(edge) {
  ui.edge = ["left", "right", "top", "bottom"].includes(edge) ? edge : "right";
  for (const option of els.edgeOptions) {
    const active = option.dataset.edge === ui.edge;
    option.classList.toggle("active", active);
    option.setAttribute("aria-checked", String(active));
    option.setAttribute("aria-label", t(EDGE_LABELS[option.dataset.edge]));
  }
  updateLayoutSummary();
}

function setDisabledControls(disabled) {
  for (const input of els.form.querySelectorAll("input")) input.disabled = disabled;
  for (const option of els.roleOptions) option.disabled = disabled;
  for (const option of els.edgeOptions) option.disabled = disabled;
  els.generatePasscode.disabled = disabled;
  els.togglePasscode.disabled = disabled;
  els.editLayout.disabled = disabled;
  els.startBtn.disabled = disabled || ui.loading;
  els.stopBtn.hidden = !disabled;
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
  const errorRaw = ui.startError || statusError || (permissionBlocked ? "errors.PERMISSIONS_BLOCKED" : null);

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

  for (const option of els.edgeOptions) {
    option.addEventListener("click", () => {
      if (ui.running) return;
      setEdge(option.dataset.edge);
    });
  }

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
