<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
  import { api, type Config, type Snapshot } from "./lib/api";
  import { activeWindow, app, isMac, sessionByKey, sidebarRows, toast, windowKey } from "./lib/store.svelte";
  import { terminals } from "./lib/terminals";
  import { matches, parseBinding } from "./lib/keys";
  import { activateRow, peekSidebar, toggleSidebar } from "./lib/actions";
  import Sidebar from "./components/Sidebar.svelte";
  import Header from "./components/Header.svelte";
  import TerminalPane from "./components/TerminalPane.svelte";
  import Switcher from "./components/Switcher.svelte";
  import Palette from "./components/Palette.svelte";
  import NewSession from "./components/NewSession.svelte";
  import RenameDialog from "./components/RenameDialog.svelte";
  import HookDialog from "./components/HookDialog.svelte";
  import ConfirmQuit from "./components/ConfirmQuit.svelte";
  import HostsDialog from "./components/HostsDialog.svelte";
  import ContextMenu from "./components/ContextMenu.svelte";
  import TermMenu from "./components/TermMenu.svelte";

  function applySnapshot(s: Snapshot) {
    const before = new Map(app.sessions.map((x) => [x.key, x.state]));
    app.hosts = s.hosts;
    app.sessions = s.sessions;
    maybeNotify(before);
  }

  async function maybeNotify(before: Map<string, string | null>) {
    if (!app.config?.notify_permission) return;
    for (const s of app.sessions) {
      if (s.state !== "permission" || before.get(s.key) === "permission" || !before.has(s.key)) continue;
      if (s.key === app.active && document.hasFocus()) continue;
      let ok = await isPermissionGranted();
      if (!ok) ok = (await requestPermission()) === "granted";
      if (ok) sendNotification({ title: "Claude needs permission", body: `${s.display_name} on ${s.host}` });
    }
  }

  function applyConfig(cfg: Config, error: string | null) {
    app.config = cfg;
    app.configError = error;
    document.documentElement.style.setProperty("--term-bg", cfg.theme.background ?? "#16181d");
    terminals.applyConfig(cfg);
    if (error) toast(error, true);
  }

  function onKey(e: KeyboardEvent) {
    const k = app.config?.keys;
    if (!k) return;
    const mac = isMac();
    const act = (fn: () => void) => {
      e.preventDefault();
      e.stopPropagation();
      fn();
    };
    if (matches(e, parseBinding(k.switcher), mac))
      return act(() => (app.overlay = app.overlay?.kind === "switcher" ? null : { kind: "switcher" }));
    if (matches(e, parseBinding(k.palette), mac))
      return act(() => (app.overlay = app.overlay?.kind === "palette" ? null : { kind: "palette" }));
    if (matches(e, parseBinding(k.new_session), mac))
      return act(() => (app.overlay = { kind: "new", sessionKind: "claude" }));
    if (matches(e, parseBinding(k.toggle_sidebar), mac)) return act(toggleSidebar);
    if (matches(e, parseBinding(k.peek_sidebar || "Ctrl+Alt+B"), mac)) return act(peekSidebar);
    // Ctrl+= / Ctrl+- / Ctrl+0 (Cmd on macOS): terminal font size.
    if ((mac ? e.metaKey : e.ctrlKey) && !e.altKey && !(mac ? e.ctrlKey : e.metaKey)) {
      if (!e.shiftKey && (e.code === "Equal" || e.code === "NumpadAdd")) return act(() => terminals.zoom(1));
      if (e.shiftKey && e.code === "Equal") return act(() => terminals.zoom(1)); // Ctrl++ on layouts where + needs Shift
      if (!e.shiftKey && (e.code === "Minus" || e.code === "NumpadSubtract")) return act(() => terminals.zoom(-1));
      if (!e.shiftKey && (e.code === "Digit0" || e.code === "Numpad0")) return act(() => terminals.zoom(0));
    }
    // Ctrl+Shift+1..9 (Cmd+Shift on macOS): the numbered rows in the sidebar.
    const digit = /^Digit([1-9])$/.exec(e.code);
    if (digit && e.shiftKey && !e.altKey && (mac ? e.metaKey : e.ctrlKey)) {
      const r = sidebarRows()[Number(digit[1]) - 1];
      return act(() => r && activateRow(r));
    }
    if (e.key === "Escape" && (app.overlay || app.menu || app.termMenu || app.sidebarPeek)) {
      return act(() => {
        if (app.overlay || app.menu || app.termMenu) {
          app.overlay = null;
          app.menu = null;
          app.termMenu = null;
        } else {
          app.sidebarPeek = false;
        }
        terminals.focusActive();
      });
    }
  }

  // Auto-hide: the sidebar slides out when the pointer rests on the left
  // edge, and goes away shortly after the pointer leaves it.
  let docked = $derived(!app.sidebarAutoHide && !app.sidebarCollapsed);
  let enterTimer: ReturnType<typeof setTimeout> | undefined;
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;
  function edgeEnter() {
    clearTimeout(enterTimer);
    enterTimer = setTimeout(() => (app.sidebarPeek = true), 120);
  }
  function edgeLeave() {
    clearTimeout(enterTimer);
  }
  function peekEnter() {
    clearTimeout(leaveTimer);
  }
  function peekLeave(e: PointerEvent) {
    // Pointer capture (resizing) or a menu opened from the sidebar keeps it out.
    if (e.buttons || app.menu || app.overlay) return;
    clearTimeout(leaveTimer);
    leaveTimer = setTimeout(() => {
      if (app.menu || app.overlay) return;
      app.sidebarPeek = false;
    }, 350);
  }

  // Leaving overlays returns focus to the terminal.
  $effect(() => {
    if (app.overlay === null) queueMicrotask(() => terminals.focusActive());
  });

  // A session viewed while "done" arrives counts as seen.
  $effect(() => {
    const s = sessionByKey(app.active);
    if (!s || !document.hasFocus()) return;
    const seen = (key: string, state: string | null, at: number) => {
      if (state === "done" && at > (app.lastViewed[key] ?? 0)) {
        app.lastViewed[key] = at;
        api.markViewed(key).catch(() => {});
      }
    };
    seen(s.key, s.state, s.state_at);
    const w = activeWindow(s);
    if (w) seen(windowKey(s, w), w.state, w.state_at);
  });

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    const unlisten: Promise<() => void>[] = [];
    (async () => {
      const b = await api.boot();
      app.platform = b.platform;
      app.lastViewed = b.state.last_viewed ?? {};
      app.knownFolders = b.state.known_folders ?? {};
      app.listMode = b.state.list_mode ?? "recent";
      app.sidebarWidth = b.state.sidebar_width || 280;
      app.sidebarCollapsed = !!b.state.sidebar_collapsed;
      app.sidebarAutoHide = !!b.state.sidebar_auto_hide;
      app.fontZoom = b.state.font_zoom ?? 0;
      applyConfig(b.config, b.config_error);
      applySnapshot(b.snapshot);
      app.ready = true;
      unlisten.push(listen<Snapshot>("snapshot", (ev) => applySnapshot(ev.payload)));
      unlisten.push(listen<{ config: Config; error: string | null }>("config", (ev) => applyConfig(ev.payload.config, ev.payload.error)));
      unlisten.push(listen<number>("confirm-quit", (ev) => (app.overlay = { kind: "confirm-quit", count: ev.payload })));
      // Known folders grow as the backend sees sessions; refresh occasionally.
      setInterval(async () => {
        try {
          app.knownFolders = (await api.getState()).known_folders ?? {};
        } catch {}
      }, 15000);
    })();
    return () => {
      window.removeEventListener("keydown", onKey, true);
      unlisten.forEach((p) => p.then((f) => f()));
    };
  });
</script>

<div class="shell" class:mac={app.platform === "macos"}>
  {#if docked}
    <Sidebar />
  {:else if app.sidebarPeek}
    <div class="peek" role="presentation" onpointerenter={peekEnter} onpointerleave={peekLeave}>
      <Sidebar floating />
    </div>
  {:else if app.sidebarAutoHide}
    <div class="edge" role="presentation" onpointerenter={edgeEnter} onpointerleave={edgeLeave}></div>
  {/if}
  <main onpointerdown={() => !docked && app.sidebarPeek && (app.sidebarPeek = false)}>
    <Header />
    <TerminalPane />
  </main>
</div>

{#if app.overlay?.kind === "switcher"}
  <Switcher />
{:else if app.overlay?.kind === "palette"}
  <Palette />
{:else if app.overlay?.kind === "new"}
  <NewSession initialKind={app.overlay.sessionKind} presetHost={app.overlay.host} />
{:else if app.overlay?.kind === "rename"}
  <RenameDialog row={app.overlay.row} />
{:else if app.overlay?.kind === "hooks"}
  <HookDialog host={app.overlay.host} install={app.overlay.install} />
{:else if app.overlay?.kind === "confirm-quit"}
  <ConfirmQuit count={app.overlay.count} />
{:else if app.overlay?.kind === "hosts"}
  <HostsDialog />
{/if}

{#if app.menu}
  <ContextMenu />
{/if}
{#if app.termMenu}
  <TermMenu />
{/if}

{#if app.toast}
  <div class="toast" class:error={app.toast.error}>{app.toast.text}</div>
{/if}

<style>
  .shell {
    position: relative;
    display: flex;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
  }
  .peek {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: 40;
    display: flex;
  }
  /* Invisible strip on the left edge that brings the sidebar out. */
  .edge {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    width: 6px;
    z-index: 39;
  }
  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--term-bg);
  }
  .toast {
    position: fixed;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--panel-2);
    border: 1px solid var(--border);
    color: var(--fg);
    padding: 8px 14px;
    border-radius: 8px;
    font-size: 12.5px;
    max-width: min(640px, 90vw);
    z-index: 100;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
    word-break: break-word;
  }
  .toast.error {
    border-color: var(--red);
  }
</style>
