(() => {
  const controls = new Map();
  let opened = null;
  const chevron = '<svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m5 6 3 3 3-3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';

  class AppSelect {
    constructor(source) {
      this.source = source;
      this.items = [];
      this.active = -1;
      this.search = "";
      this.searchAt = 0;
      this.signature = "";
      this.root = document.createElement("div");
      this.root.className = "app-select";
      this.root.dataset.control = source.id;
      source.before(this.root);
      this.root.append(source);
      source.hidden = true;
      this.button = document.createElement("button");
      this.button.type = "button";
      this.button.className = "select-trigger";
      this.button.id = `${source.id}-trigger`;
      this.button.setAttribute("role", "combobox");
      this.button.setAttribute("aria-haspopup", "listbox");
      this.button.setAttribute("aria-expanded", "false");
      this.value = document.createElement("span");
      this.value.className = "select-value";
      this.value.id = `${source.id}-value`;
      this.button.append(this.value);
      this.button.insertAdjacentHTML("beforeend", chevron);
      this.root.append(this.button);
      this.menu = document.createElement("div");
      this.menu.className = "select-menu";
      this.menu.id = `${source.id}-menu`;
      this.menu.hidden = true;
      this.menu.setAttribute("role", "listbox");
      this.button.setAttribute("aria-controls", this.menu.id);
      const label = document.querySelector(`label[for="${source.id}"]`);
      if (label) {
        label.id ||= `${source.id}-label`;
        label.htmlFor = this.button.id;
        this.button.setAttribute("aria-labelledby", `${label.id} ${this.value.id}`);
        this.menu.setAttribute("aria-labelledby", label.id);
      }
      document.body.append(this.menu);
      this.button.addEventListener("click", () => opened === this ? this.close() : this.open());
      this.button.addEventListener("keydown", (event) => this.onKey(event));
      this.menu.addEventListener("pointerdown", (event) => event.preventDefault());
      source.addEventListener("change", () => this.sync());
      new MutationObserver(() => this.sync()).observe(source, {
        attributes: true, childList: true, characterData: true, subtree: true,
      });
      this.sync();
    }

    sync() {
      const options = Array.from(this.source.options);
      const signature = JSON.stringify(options.map((option) => [option.value, option.label, option.disabled]));
      const optionsChanged = signature !== this.signature;
      if (optionsChanged) {
        this.signature = signature;
        this.menu.replaceChildren();
        this.items = options.map((option, index) => {
          const item = document.createElement("div");
          item.className = "select-option";
          item.id = `${this.source.id}-option-${index}`;
          item.setAttribute("role", "option");
          item.setAttribute("aria-disabled", String(option.disabled));
          item.textContent = option.label;
          item.title = option.label;
          item.addEventListener("pointermove", () => {
            if (!option.disabled) this.setActive(index, false);
          });
          item.addEventListener("click", () => this.choose(index));
          this.menu.append(item);
          return { element: item, value: option.value, disabled: option.disabled };
        });
      }
      this.button.disabled = this.source.disabled || !this.items.some((item) => !item.disabled);
      this.value.textContent = this.source.selectedOptions[0]?.label || "—";
      this.button.title = this.value.textContent;
      if (!this.button.hasAttribute("aria-labelledby")) {
        this.button.setAttribute("aria-label", this.source.getAttribute("aria-label") || this.value.textContent);
      }
      this.items.forEach((item) => {
        const selected = item.value === this.source.value;
        item.element.setAttribute("aria-selected", String(selected));
      });
      if (opened === this) {
        if (this.button.disabled || !this.button.getClientRects().length) this.close();
        else {
          if (optionsChanged || !this.items[this.active] || this.items[this.active].disabled) this.setActive(this.selected());
          this.position();
        }
      }
    }

    selected() {
      const selected = this.items.findIndex((item) => item.value === this.source.value && !item.disabled);
      return selected >= 0 ? selected : this.items.findIndex((item) => !item.disabled);
    }

    open() {
      this.sync();
      if (this.button.disabled) return;
      opened?.close();
      opened = this;
      this.menu.hidden = false;
      this.button.setAttribute("aria-expanded", "true");
      this.search = "";
      this.position();
      this.setActive(this.selected());
      this.button.focus({ preventScroll: true });
    }

    close() {
      this.menu.hidden = true;
      this.button.setAttribute("aria-expanded", "false");
      this.button.removeAttribute("aria-activedescendant");
      if (opened === this) opened = null;
    }

    position() {
      const rect = this.button.getBoundingClientRect();
      const margin = 10;
      this.menu.style.width = `${Math.min(Math.max(rect.width, 180), window.innerWidth - margin * 2)}px`;
      const below = window.innerHeight - rect.bottom - margin - 5;
      const above = rect.top - margin - 5;
      const upward = below < Math.min(this.menu.scrollHeight, 180) && above > below;
      this.menu.style.maxHeight = `${Math.max(40, Math.min(280, upward ? above : below))}px`;
      this.menu.style.left = `${Math.max(margin, Math.min(rect.right - this.menu.offsetWidth, window.innerWidth - this.menu.offsetWidth - margin))}px`;
      this.menu.style.top = `${upward ? rect.top - this.menu.offsetHeight - 5 : rect.bottom + 5}px`;
    }

    setActive(index, scroll = true) {
      this.active = index;
      this.items.forEach((item, i) => item.element.classList.toggle("is-active", i === index));
      const item = this.items[index];
      if (item) {
        this.button.setAttribute("aria-activedescendant", item.element.id);
        if (scroll) item.element.scrollIntoView({ block: "nearest" });
      }
    }

    choose(index) {
      const item = this.items[index];
      if (!item || item.disabled || this.button.disabled) return;
      const changed = this.source.value !== item.value;
      this.source.value = item.value;
      this.close();
      this.sync();
      this.button.focus({ preventScroll: true });
      if (changed) this.source.dispatchEvent(new Event("change", { bubbles: true }));
    }

    onKey(event) {
      const isOpen = opened === this;
      if (event.key === "Tab") { this.close(); return; }
      if (event.key === "Escape") {
        if (!isOpen) return;
        event.preventDefault(); event.stopPropagation(); this.close(); return;
      }
      if (["Enter", " ", "ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault(); event.stopPropagation();
        if (!isOpen) this.open();
        if (["Enter", " "].includes(event.key)) {
          if (isOpen) this.choose(this.active);
          return;
        }
        const enabled = this.items.map((item, index) => item.disabled ? -1 : index).filter((index) => index >= 0);
        if (!enabled.length) return;
        let next = enabled.indexOf(this.active);
        if (event.key === "Home") next = 0;
        else if (event.key === "End") next = enabled.length - 1;
        else if (isOpen) next = (next + (event.key === "ArrowDown" ? 1 : -1) + enabled.length) % enabled.length;
        this.setActive(enabled[next]);
      } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
        event.preventDefault(); event.stopPropagation();
        if (!isOpen) this.open();
        const now = Date.now();
        this.search = (now - this.searchAt < 700 ? this.search : "") + event.key.toLocaleLowerCase();
        this.searchAt = now;
        const index = this.items.findIndex((item) => !item.disabled && item.element.textContent.toLocaleLowerCase().startsWith(this.search));
        if (index >= 0) this.setActive(index);
      }
    }
  }

  document.querySelectorAll("select").forEach((source) => controls.set(source, new AppSelect(source)));
  document.addEventListener("pointerdown", (event) => {
    if (opened && !opened.root.contains(event.target) && !opened.menu.contains(event.target)) opened.close();
  });
  document.addEventListener("scroll", (event) => {
    if (opened && !opened.menu.contains(event.target)) opened.position();
  }, true);
  window.addEventListener("resize", () => opened?.position());
  window.addEventListener("blur", () => opened?.close());
  document.addEventListener("rx-switch:locale", () => {
    opened?.close();
    controls.forEach((control) => control.sync());
  });
  window.AppSelect = {
    sync: (source) => source ? controls.get(source)?.sync() : controls.forEach((control) => control.sync()),
    close: () => opened?.close(),
  };
})();
