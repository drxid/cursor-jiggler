<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import {
    enable as autostartEnable,
    disable as autostartDisable,
    isEnabled as autostartIsEnabled,
  } from "@tauri-apps/plugin-autostart";
  import { load } from "@tauri-apps/plugin-store";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { check } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import { getVersion } from "@tauri-apps/api/app";
  import { onMount, tick } from "svelte";
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { LANGS, translations, updateT, detectLang, type Lang } from "./i18n";

  type Status = {
    enabled: boolean;
    zen: boolean;
    interval: number;
    remaining: number;
    startMinimized: boolean;
  };

  let enabled = $state(false);
  let zen = $state(false);
  let interval = $state(60);
  let remaining = $state(60);
  let onTop = $state(false);
  let autostart = $state(false);
  let startMinimized = $state(false);
  let view = $state<"main" | "settings" | "language">("main");
  let lang = $state<Lang>("en");
  let updateState = $state<
    "idle" | "checking" | "available" | "downloading" | "uptodate" | "error"
  >("idle");
  let updateVersion = $state("");
  let appVersion = $state("");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let pendingUpdate: any = null;

  const t = $derived(translations[lang]);
  const u = $derived(updateT[lang]);

  const isMac =
    typeof navigator !== "undefined" && /Mac OS X|Macintosh/.test(navigator.userAgent);

  const presetValues = [30, 60, 180, 300];
  const presetLabel = (v: number) =>
    v < 60 ? `${v} ${t.secShort}` : `${v / 60} ${t.minShort}`;

  const appWin = getCurrentWindow();
  const fmt = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  // View navigation with a direction-aware slide (deeper = slide in from the right).
  const depth = { main: 0, settings: 1, language: 2 };
  let navDir = $state(1);
  function go(target: "main" | "settings" | "language") {
    navDir = depth[target] >= depth[view] ? 1 : -1;
    view = target;
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let store: any = null;

  function apply(s: Status) {
    enabled = s.enabled;
    zen = s.zen;
    interval = s.interval;
    remaining = s.remaining;
    startMinimized = s.startMinimized;
  }

  // Tray strings for the Rust side, built from the current language.
  function trayLabels() {
    return {
      status_active: "● " + t.statusActive,
      status_inactive: "○ " + t.statusInactive,
      enable: t.trayEnable,
      disable: t.trayDisable,
      show: t.trayShow,
      quit: t.trayQuit,
      tooltip_active: t.tooltipActive,
      tooltip_inactive: t.tooltipInactive,
    };
  }
  async function pushLabels() {
    try {
      await invoke("set_labels", { labels: trayLabels() });
    } catch (_) {
      /* tray not ready yet */
    }
  }

  // Apply language to the document, persist it, and localize the tray.
  $effect(() => {
    const meta = LANGS.find((l) => l.code === lang);
    document.documentElement.lang = lang;
    document.documentElement.dir = meta?.rtl ? "rtl" : "ltr";
    store?.set("lang", lang);
    pushLabels();
  });

  // Re-fit the window height whenever the view changes.
  $effect(() => {
    view;
    requestAnimationFrame(fitWindow);
  });

  // Shrink the OS window to exactly wrap the card — no empty space, no shadow.
  async function fitWindow() {
    await tick();
    const el = document.querySelector(".window") as HTMLElement | null;
    if (!el) return;
    const h = Math.ceil(el.getBoundingClientRect().height);
    await appWin.setSize(new LogicalSize(420, h));
  }

  onMount(async () => {
    apply(await invoke<Status>("get_status"));
    await listen<Status>("status", (e) => apply(e.payload));
    try {
      autostart = await autostartIsEnabled();
    } catch (_) {
      /* plugin unavailable in some environments */
    }
    getVersion()
      .then((v) => (appVersion = v))
      .catch(() => {});

    // Language: restore the saved choice or fall back to the system locale.
    try {
      store = await load("ui.json", { autoSave: 200 });
      const saved = (await store.get("lang")) as Lang | null;
      lang = saved && translations[saved] ? saved : detectLang();
    } catch (_) {
      lang = detectLang();
    }

    // Re-fit the window whenever the card's height changes (theme, content, HMR).
    const el = document.querySelector(".window");
    if (el && "ResizeObserver" in window) {
      new ResizeObserver(() => fitWindow()).observe(el);
    }
    requestAnimationFrame(fitWindow);

    // Kill any stuck hover when the window (re)gains focus, e.g. opened from tray.
    suppressHoverUntilMove();
    appWin.onFocusChanged(({ payload: focused }) => {
      if (focused) suppressHoverUntilMove();
    });

    // Quietly check for updates on launch.
    checkUpdate(false);
  });

  async function toggleEnabled() {
    apply(await invoke<Status>("set_enabled", { enabled: !enabled }));
  }
  async function toggleZen() {
    apply(await invoke<Status>("set_zen", { zen: !zen }));
  }
  async function pick(v: number) {
    apply(await invoke<Status>("set_interval", { seconds: v }));
  }
  async function toggleOnTop() {
    onTop = !onTop;
    await appWin.setAlwaysOnTop(onTop);
  }
  async function toggleStartMinimized() {
    apply(
      await invoke<Status>("set_start_minimized", { value: !startMinimized }),
    );
  }
  async function toggleAutostart() {
    try {
      if (autostart) await autostartDisable();
      else await autostartEnable();
      autostart = await autostartIsEnabled();
    } catch (e) {
      console.error("autostart toggle failed", e);
    }
  }

  async function checkUpdate(manual = false) {
    if (updateState === "checking" || updateState === "downloading") return;
    updateState = "checking";
    try {
      const upd = await check();
      if (upd) {
        pendingUpdate = upd;
        updateVersion = upd.version;
        updateState = "available";
      } else {
        updateState = manual ? "uptodate" : "idle";
      }
    } catch (e) {
      console.error("update check failed", e);
      updateState = manual ? "error" : "idle";
    }
  }
  async function installUpdate() {
    if (!pendingUpdate) return;
    updateState = "downloading";
    try {
      await pendingUpdate.downloadAndInstall();
      await relaunch();
    } catch (e) {
      console.error("update install failed", e);
      updateState = "error";
    }
  }
  function onUpdateRow() {
    if (updateState === "available") installUpdate();
    else checkUpdate(true);
  }

  // Prevent a "stuck" hover state when the window is shown from the tray.
  function suppressHoverUntilMove() {
    document.documentElement.classList.add("suppress-hover");
    // Also drop any lingering keyboard-focus ring left over from before hiding.
    (document.activeElement as HTMLElement | null)?.blur();
    const clear = () => {
      document.documentElement.classList.remove("suppress-hover");
      window.removeEventListener("mousemove", clear);
    };
    window.addEventListener("mousemove", clear);
  }
</script>

<main class="window">
  <!-- Custom titlebar: traffic lights (left) on macOS, caption buttons (right) on Windows/Linux -->
  <div class="titlebar" data-tauri-drag-region>
    {#if isMac}
      <div class="mac-lights">
        <button class="light red" aria-label="close" onclick={() => appWin.hide()}></button>
        <button class="light yellow" aria-label="minimize" onclick={() => appWin.minimize()}></button>
        <span class="light green"></span>
      </div>
    {/if}

    <button
      class="icon-btn gear {isMac ? 'mac' : 'win'}"
      title={view === "main" ? t.settings : t.back}
      aria-label={view === "main" ? t.settings : t.back}
      onclick={() =>
        go(view === "main" ? "settings" : view === "language" ? "settings" : "main")}
    >
      {#if view !== "main"}
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" />
          <path d="M12 19l-7-7 7-7" />
        </svg>
      {:else}
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      {/if}
    </button>

    <div class="title">
      {view === "main" ? "Cursor Jiggler" : view === "language" ? t.language : t.settings}
    </div>

    {#if !isMac}
      <div class="win-controls">
        <button class="win-btn" aria-label="minimize" onclick={() => appWin.minimize()}>
          <svg width="11" height="11" viewBox="0 0 11 11"><rect x="1" y="5" width="9" height="1" fill="currentColor" /></svg>
        </button>
        <button class="win-btn close" aria-label="close" onclick={() => appWin.hide()}>
          <svg width="11" height="11" viewBox="0 0 11 11"
            ><path d="M1 1 L10 10 M10 1 L1 10" stroke="currentColor" stroke-width="1.1" /></svg
          >
        </button>
      </div>
    {/if}
  </div>

  <div class="content">
    {#key view}
      <div
        class="view-wrap"
        in:fly={{ x: navDir * 24, duration: 200, opacity: 0, easing: cubicOut }}
      >
        {#if view === "main"}
          <!-- Power + Zen group (Zen appears only while active) -->
      <div class="card group">
        <button class="row" onclick={toggleEnabled}>
          <div class="power-left">
            <span class="dot {enabled ? 'live' : ''}"></span>
            <div class="labels">
              <span class="primary">{enabled ? t.statusActive : t.statusInactive}</span>
              <span class="secondary">{enabled ? t.movingDesc : t.pressToStart}</span>
            </div>
          </div>
          <span class="switch {enabled ? 'checked' : ''}"><span class="knob"></span></span>
        </button>
        {#if enabled}
          <button class="row sub" onclick={toggleZen}>
            <div class="labels">
              <span class="primary">{t.zen}</span>
              <span class="secondary">{t.zenDesc}</span>
            </div>
            <span class="switch {zen ? 'checked' : ''}"><span class="knob"></span></span>
          </button>
        {/if}
      </div>

      <!-- Frequency -->
      <div class="section-label">{t.frequency}</div>
      <div class="presets">
        {#each presetValues as v}
          <button
            class="preset {interval === v ? 'active' : ''}"
            onclick={() => pick(v)}
          >
            {presetLabel(v)}
          </button>
        {/each}
      </div>
    {:else if view === "settings"}
      <!-- Settings -->
      <button class="card slim" onclick={toggleOnTop}>
        <div class="labels">
          <span class="primary">{t.onTop}</span>
          <span class="secondary">{t.onTopDesc}</span>
        </div>
        <span class="switch {onTop ? 'checked' : ''}"><span class="knob"></span></span>
      </button>

      <div class="card group">
        <button class="row" onclick={toggleAutostart}>
          <div class="labels">
            <span class="primary">{t.autostart}</span>
            <span class="secondary">{t.autostartDesc}</span>
          </div>
          <span class="switch {autostart ? 'checked' : ''}"><span class="knob"></span></span>
        </button>
        {#if autostart}
          <button class="row sub" onclick={toggleStartMinimized}>
            <div class="labels">
              <span class="primary">{t.startMinimized}</span>
              <span class="secondary">{t.startMinimizedDesc}</span>
            </div>
            <span class="switch {startMinimized ? 'checked' : ''}"><span class="knob"></span></span>
          </button>
        {/if}
      </div>

      <button class="card slim lang-row" onclick={() => go("language")}>
        <span class="primary">{t.language}</span>
        <span class="lang-current">
          {LANGS.find((l) => l.code === lang)?.name}
          <svg class="chev-right" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9 6l6 6-6 6" />
          </svg>
        </span>
      </button>

      <button
        class="card slim update-row"
        onclick={onUpdateRow}
        disabled={updateState === "checking" || updateState === "downloading"}
      >
        <span class="primary">
          {updateState === "available"
            ? u.available
            : updateState === "uptodate"
              ? u.upToDate
              : updateState === "error"
                ? u.error
                : u.check}
        </span>
        <span class="update-action {updateState === 'available' ? 'accent' : ''}">
          {#if updateState === "checking"}{u.checking}
          {:else if updateState === "downloading"}{u.downloading}
          {:else if updateState === "available"}v{updateVersion} · {u.install}{/if}
        </span>
      </button>

      <div class="note">{t.note}</div>

      <div class="footer">
        Cursor Jiggler{appVersion ? ` · v${appVersion}` : ""} · <button class="link" onclick={() => openUrl("https://drxid.ru/")}>drxid</button>
      </div>
    {:else}
      <!-- Language selection (scrolls; never stretches the window) -->
      <div class="lang-list-full">
        {#each LANGS as l}
          <button
            class="lang-item {l.code === lang ? 'active' : ''}"
            onclick={() => {
              lang = l.code;
              go("settings");
            }}
          >
            {l.name}
            {#if l.code === lang}
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 6L9 17l-5-5" />
              </svg>
            {/if}
          </button>
        {/each}
      </div>
        {/if}
      </div>
    {/key}
  </div>

  <!-- Status bar -->
  <div class="statusbar">
    <span class="status-dot {enabled ? 'live' : ''}"></span>
    <span class="status-text">{enabled ? t.running : t.ready}</span>
    <span class="status-right">
      {enabled ? `${t.nextIn} ${fmt(remaining)}` : `${t.intervalLabel} ${fmt(interval)}`}
    </span>
  </div>
</main>

<style>
  .window {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 14px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    color: var(--text);
  }

  /* Titlebar */
  .titlebar {
    position: relative;
    height: 34px;
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
  .title {
    position: absolute;
    left: 0;
    right: 0;
    text-align: center;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-dim);
    pointer-events: none;
  }

  /* macOS traffic lights */
  .mac-lights {
    position: absolute;
    left: 14px;
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .light {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: none;
    padding: 0;
    cursor: pointer;
  }
  .light.red {
    background: #ff5f57;
  }
  .light.yellow {
    background: #febc2e;
  }
  .light.green {
    background: #28c840;
    cursor: default;
  }

  /* Windows caption buttons */
  .win-controls {
    position: absolute;
    right: 0;
    top: 0;
    height: 100%;
    display: flex;
  }
  .win-btn {
    width: 44px;
    height: 100%;
    border: none;
    background: transparent;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition:
      background 0.12s ease,
      color 0.12s ease;
  }
  .win-btn:hover {
    background: var(--ctl-hover);
    color: var(--text);
  }
  .win-btn.close:hover {
    background: #e81123;
    color: #fff;
  }
  /* Top-right rounding follows the window corner */
  .win-btn.close {
    border-top-right-radius: 13px;
  }

  /* Gear / settings button */
  .icon-btn {
    position: absolute;
    top: 0;
    height: 100%;
    width: 44px;
    border: none;
    background: transparent;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition:
      background 0.12s ease,
      color 0.12s ease;
  }
  .icon-btn:hover {
    background: var(--ctl-hover);
    color: var(--text);
  }
  /* Flush to the window edge, matching the caption buttons' hit area */
  .gear.win {
    left: 0;
    border-top-left-radius: 13px;
  }
  .gear.mac {
    right: 0;
    border-top-right-radius: 13px;
  }

  /* Content */
  .content {
    padding: 10px 16px 16px;
  }
  .view-wrap {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .card {
    width: 100%;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 15px 16px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    transition:
      background 0.15s ease,
      transform 0.08s ease,
      box-shadow 0.15s ease;
  }
  .card.slim {
    padding: 12px 16px;
  }
  .card:hover {
    background: var(--card-hover);
  }
  .card:active {
    transform: scale(0.99);
  }

  /* Grouped card: several toggle rows sharing one card (e.g. autostart + sub-option) */
  .card.group {
    padding: 0;
    overflow: hidden;
    display: block;
  }
  .card.group:hover {
    background: var(--card);
  }
  .card.group:active {
    transform: none;
  }
  .group .row {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: transparent;
    border: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .group .row:hover {
    background: var(--card-hover);
  }
  .group .row.sub {
    border-top: 1px solid var(--border);
  }
  .group .row.sub .primary {
    font-size: 13.5px;
  }
  /* Inset focus ring so it isn't clipped by the card's overflow:hidden */
  .group .row:focus-visible {
    outline-offset: -3px;
  }

  /* Language row in settings (opens the language sub-view) */
  .lang-current {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--text-dim);
    font-size: 13px;
    font-weight: 500;
  }

  /* Update row */
  .update-action {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-dim);
  }
  .update-action.accent {
    color: var(--accent);
    font-weight: 600;
  }
  .card:disabled {
    opacity: 0.65;
    cursor: default;
  }

  /* Language sub-view: a scrollable list that never stretches the window. */
  .lang-list-full {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 300px;
    overflow-y: auto;
    padding: 2px;
  }
  .lang-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    text-align: start;
    background: var(--card);
    border: 1px solid var(--border);
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    padding: 12px 14px;
    border-radius: 10px;
    cursor: pointer;
    transition:
      background 0.12s ease,
      color 0.12s ease;
  }
  .lang-item:hover {
    background: var(--card-hover);
  }
  .lang-item.active {
    color: var(--accent);
    border-color: var(--accent);
    background: var(--accent-soft);
    font-weight: 600;
  }
  .lang-list-full::-webkit-scrollbar {
    width: 8px;
  }
  .lang-list-full::-webkit-scrollbar-thumb {
    background: var(--track);
    border-radius: 4px;
  }
  .lang-list-full::-webkit-scrollbar-track {
    background: transparent;
  }

  .power-left {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-dim);
    flex-shrink: 0;
    transition: background 0.2s ease;
  }
  .dot.live {
    background: var(--green);
    box-shadow: 0 0 10px var(--green);
  }

  .labels {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .primary {
    font-size: 14.5px;
    font-weight: 600;
    color: var(--text);
  }
  .secondary {
    font-size: 11.5px;
    color: var(--text-dim);
  }

  /* Switch */
  .switch {
    width: 44px;
    height: 26px;
    border-radius: 13px;
    background: var(--track);
    flex-shrink: 0;
    position: relative;
    transition: background 0.2s ease;
  }
  .switch.checked {
    background: var(--accent);
  }
  .knob {
    position: absolute;
    top: 3px;
    inset-inline-start: 3px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
    transition: transform 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .switch.checked .knob {
    transform: translateX(18px);
  }
  :global(html[dir="rtl"]) .switch.checked .knob {
    transform: translateX(-18px);
  }

  /* Frequency */
  .section-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim);
    margin: 4px 2px -2px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .presets {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 7px;
  }
  .preset {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 9px 4px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background 0.15s ease,
      color 0.15s ease,
      transform 0.08s ease;
  }
  .preset:hover {
    background: var(--card-hover);
  }
  .preset:active {
    transform: scale(0.96);
  }
  .preset.active {
    background: var(--accent-soft);
    color: var(--accent);
    border-color: var(--accent);
  }

  /* Settings */
  .note {
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-dim);
    padding: 2px;
  }
  .footer {
    font-size: 11px;
    color: var(--text-dim);
    padding: 2px;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }

  /* Statusbar */
  .statusbar {
    height: 42px;
    flex-shrink: 0;
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px;
    font-size: 12px;
  }
  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .status-dot.live {
    background: var(--green);
  }
  .status-text {
    color: var(--text);
    font-weight: 500;
  }
  .status-right {
    margin-inline-start: auto;
    color: var(--text-dim);
  }
</style>
