(() => {
  const invoke = window.__TAURI__?.core?.invoke;
  const i18n = window.rxSwitchI18n;
  const t = (key) => i18n.t(key);
  const build = document.getElementById("buildValue");
  const system = document.getElementById("systemValue");
  const copy = document.getElementById("copyAbout");
  const close = document.getElementById("closeAbout");
  const status = document.getElementById("copyStatus");

  async function closeWindow() {
    if (invoke) await invoke("close_about");
  }

  close.addEventListener("click", () => closeWindow().catch((error) => { status.textContent = String(error); }));
  copy.addEventListener("click", async () => {
    const text = `rx-switch\n${t("about.version")}\n${t("about.build")}: ${build.textContent}\n${t("about.system")}: ${system.textContent}`;
    let copied = false;
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch {
      const temporary = document.createElement("textarea");
      temporary.value = text;
      temporary.style.position = "fixed";
      temporary.style.opacity = "0";
      document.body.append(temporary);
      temporary.select();
      try {
        copied = document.execCommand("copy");
      } catch {
        copied = false;
      } finally {
        temporary.remove();
        copy.focus();
      }
    }
    status.textContent = t(copied ? "about.copied" : "about.copyFailed");
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" || (event.key === "Enter" && event.target === document.body)) {
      event.preventDefault();
      closeWindow().catch((error) => { status.textContent = String(error); });
    }
  });

  async function boot() {
    if (!invoke) return;
    try {
      const state = await invoke("get_state");
      if (["zh", "en"].includes(state.config?.language)) i18n.setLocale(state.config.language);
      build.textContent = state.build || "—";
      system.textContent = { macos: "macOS", windows: "Windows" }[state.platform] || state.platform || "—";
      document.title = t("about.title");
      close.focus();
    } catch (error) {
      status.textContent = String(error);
    }
  }
  boot();
})();
