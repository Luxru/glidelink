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

  let preparing = null;
  function prepare() {
    if (preparing) return preparing;
    preparing = prepareContent().finally(() => { preparing = null; });
    return preparing;
  }

  async function prepareContent() {
    status.textContent = "";
    try {
      const info = await invoke("get_about");
      if (["zh", "en"].includes(info.language)) i18n.setLocale(info.language);
      build.textContent = info.build || "—";
      system.textContent = { macos: "macOS", windows: "Windows" }[info.platform] || info.platform || "—";
      document.title = t("about.title");
      const icon = document.querySelector(".about-app-icon");
      await Promise.all([document.fonts.ready, icon.decode().catch(() => {})]);
    } catch (error) {
      status.textContent = String(error);
    }
    await invoke("about_ready");
    close.focus();
  }

  async function boot() {
    if (!invoke) return;
    await window.__TAURI__.event.listen("rx-switch:about-open", () => {
      prepare().catch((error) => { status.textContent = String(error); });
    });
    await prepare();
  }
  boot().catch((error) => { status.textContent = String(error); });
})();
