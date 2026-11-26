import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, type HostEntry, type HostInfo, type SavedSession, type Session, type Window } from "./api";
import {
  app,
  hostInfo,
  isReachable,
  rowName,
  setSidebar,
  setSidebarAutoHide,
  toast,
  visibleRows,
  windowTarget,
  type Row,
} from "./store.svelte";
import { terminals } from "./terminals";
import { isLive } from "./hostdot";

export function activate(s: Session) {
  app.overlay = null;
  app.menu = null;
  app.sidebarPeek = false;
  if (!s.persistent && !terminals.has(s.key)) {
    toast("This ephemeral session is not open in this window", true);
    return;
  }
  if (s.persistent && !isReachable(s) && !terminals.has(s.key)) {
    toast(`${s.host} is not reachable`, true);
    return;
  }
  terminals.open(s);
}

/** Shows a session's terminal and makes one of its windows current. */
export async function activateWindow(s: Session, w: Window) {
  if (!isReachable(s)) {
    toast(`${s.host} is not reachable`, true);
    return;
  }
  if (!w.active) {
    // Reflect the switch at once; the next poll confirms it.
    for (const x of s.windows) x.active = x.id === w.id;
    api.selectWindow(s.host, s.slug, w.id).catch((e) => toast(String(e), true));
  }
  activate(s);
}

export function activateRow(r: Row) {
  if (r.w) activateWindow(r.s, r.w);
  else activate(r.s);
}

/** Kills a window; the last window of a session takes the session with it. */
export async function killRow(r: Row) {
  if (!r.w || r.s.windows.length <= 1) return killSession(r.s);
  app.menu = null;
  try {
    await api.killWindow(r.s.host, r.s.slug, r.w.id);
  } catch (e) {
    toast(String(e), true);
  }
}

/** Renames the tmux window on the server, so tmux shows the name too. */
export async function renameRow(r: Row, name: string) {
  const n = name.trim();
  if (!r.w) return renameSession(r.s, n);
  if (!n || n === r.w.name) return;
  try {
    await api.renameWindow(r.s.host, r.s.slug, r.w.id, n);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function killSession(s: Session) {
  app.menu = null;
  try {
    terminals.dispose(s.key);
    await api.killSession(s.host, s.slug);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function renameSession(s: Session, name: string) {
  const n = name.trim();
  if (!n || n === s.display_name) return;
  try {
    await api.renameSession(s.host, s.slug, n);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function copyAttachCommand(s: Session) {
  app.menu = null;
  try {
    const cmd = await api.attachCommandText(s.host, s.slug);
    await writeText(cmd);
    toast("Copied: " + cmd);
  } catch (e) {
    toast(String(e), true);
  }
}

/** Creates and shows a session; returns the key of the terminal it is in. */
export async function createSession(
  host: string,
  dir: string,
  name: string,
  kind: "claude" | "shell",
): Promise<string | undefined> {
  const h = hostInfo(host);
  if (!h) {
    toast(`Unknown host ${host}`, true);
    return;
  }
  app.overlay = null;
  if (h.kind === "ephemeral") return terminals.openEphemeral(host, dir, name, kind);
  if (!isLive(h)) {
    toast(`${host} is not available: ${h.status}`, true);
    return;
  }
  const target = windowTarget(host);
  if (target) {
    try {
      const id = await api.createWindow(host, target.slug, dir, name, kind);
      // The backend refreshes before returning; mark the new window current
      // in case that poll raced the switch.
      const s = app.sessions.find((x) => x.key === target.key) ?? target;
      for (const w of s.windows) w.active = w.id === id;
      terminals.open(s);
      return s.key;
    } catch (e) {
      toast(String(e), true);
    }
    return;
  }
  try {
    const slug = await api.createSession(host, dir, name, kind);
    const key = `${host}/${slug}`;
    // The backend refreshes the list before returning; fall back to a stub.
    const s =
      app.sessions.find((x) => x.key === key) ??
      ({
        key,
        host,
        slug,
        display_name: name,
        kind,
        cwd: dir,
        state: null,
        state_at: 0,
        created: Date.now() / 1000,
        last_attached: 0,
        attached: 0,
        persistent: true,
        ephemeral_id: null,
        ended: false,
        windows: [],
      } as Session);
    terminals.open(s);
    return key;
  } catch (e) {
    toast(String(e), true);
  }
}

// Saved sessions: [[saved]] in config.toml.

export const savedSessions = (): SavedSession[] => app.config?.saved ?? [];

const sameName = (a: string, b: string) => a.trim().toLocaleLowerCase() === b.trim().toLocaleLowerCase();

export function findSaved(host: string, name: string): SavedSession | undefined {
  return savedSessions().find((x) => x.host === host && sameName(x.name, name));
}

/** A session (or tmux window) of this name is already running on its host. */
export function isSavedRunning(sv: SavedSession): boolean {
  return visibleRows().some((r) => r.s.host === sv.host && !r.s.ended && sameName(rowName(r), sv.name));
}

/** Adds a saved session, replacing `replacing` and any of the same name on the same host. */
export async function saveSession(sv: SavedSession, replacing?: SavedSession) {
  const list = savedSessions().filter((x) => x !== replacing && !(x.host === sv.host && sameName(x.name, sv.name)));
  // Saving is not a request to start it now, whatever its autostart says.
  autoTried.add(autoId(sv));
  try {
    await api.setSaved([...list, sv]);
    toast(`Saved "${sv.name}". Start it again from the palette: Start saved sessions`);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function removeSaved(sv: SavedSession) {
  try {
    await api.setSaved(savedSessions().filter((x) => x !== sv));
    toast(`Removed saved session "${sv.name}"`);
  } catch (e) {
    toast(String(e), true);
  }
}

/** Starts one saved session and types its commands. False if it did not start. */
async function startOne(sv: SavedSession): Promise<boolean> {
  const key = await createSession(sv.host, sv.cwd, sv.name, sv.kind === "claude" ? "claude" : "shell");
  if (!key) return false;
  const lines = sv.commands.filter((c) => c.trim());
  if (lines.length) terminals.runLines(key, lines);
  return true;
}

/** Starts the given saved sessions, skipping those already running. */
export async function startSaved(list: SavedSession[]) {
  app.overlay = null;
  let started = 0;
  let running = 0;
  for (const sv of list) {
    if (isSavedRunning(sv)) running++;
    else if (await startOne(sv)) started++;
  }
  const parts: string[] = [];
  if (started) parts.push(`Started ${started}`);
  if (running) parts.push(`${running} already running`);
  if (parts.length) toast(parts.join(", "));
}

/** "host/name" of saved sessions autostart has dealt with in this window. */
const autoTried = new Set<string>();
const autoId = (sv: SavedSession) => `${sv.host}/${sv.name.trim().toLocaleLowerCase()}`;

/** Starts autostart sessions once per window, as each host becomes usable.
 *  Called whenever hosts or config change. */
export function autostartSaved() {
  for (const sv of savedSessions()) {
    if (!sv.autostart || autoTried.has(autoId(sv))) continue;
    const h = hostInfo(sv.host);
    if (!h || !(isLive(h) || h.kind === "ephemeral")) continue;
    autoTried.add(autoId(sv));
    // Give the host's session list a moment to arrive, so a session that is
    // still running (keeper sessions outlive the window) is not started twice.
    setTimeout(() => {
      if (!isSavedRunning(sv)) startOne(sv);
    }, 2000);
  }
}

export function hostAction(h: HostInfo) {
  if (h.status.startsWith("needs key")) {
    // The built-in client has no key the host accepts: show the key and
    // the host's details.
    manageHosts(h.alias);
  } else if (h.needs_login) {
    terminals.openLogin(h.alias);
  } else {
    reconnectHost(h.alias);
  }
}

export async function reconnectHost(alias: string) {
  const dormant = hostInfo(alias)?.dormant;
  try {
    await api.reconnectHost(alias);
    toast(`${dormant ? "Connecting" : "Reconnecting"} ${alias}`);
  } catch (e) {
    toast(String(e), true);
  }
}

/** The host list as config.toml has it, in sidebar order. */
export function hostEntries(): HostEntry[] {
  return app.hosts.map((h) => ({ name: h.alias, auto_connect: h.auto_connect }));
}

export async function saveHosts(list: HostEntry[]) {
  try {
    await api.setHosts(list);
  } catch (e) {
    toast(String(e), true);
  }
}

/** Takes a host out of the list; its sessions on the host are not touched. */
export async function removeHost(alias: string) {
  await saveHosts(hostEntries().filter((h) => h.name !== alias));
  toast(`Removed ${alias} from the list`);
}

export async function openSettings() {
  // Android has no text editor to hand the file to: edit it in the app.
  if (app.platform === "android") {
    app.overlay = { kind: "settings" };
    return;
  }
  try {
    await api.openConfig();
  } catch (e) {
    toast(String(e), true);
  }
}

export function manageHosts(host?: string) {
  app.sidebarPeek = false;
  app.overlay = { kind: "hosts", host };
}

export async function openSshConfig() {
  try {
    await api.openSshConfig();
  } catch (e) {
    toast(String(e), true);
  }
}

/** In auto-hide mode this shows or hides the sidebar over the terminal;
 *  otherwise it docks or collapses it. */
export function toggleSidebar() {
  if (app.sidebarAutoHide) {
    app.sidebarPeek = !app.sidebarPeek;
    return;
  }
  app.sidebarPeek = false;
  setSidebar(!app.sidebarCollapsed);
  setTimeout(() => terminals.fitActive(), 50);
}

/** Brings the sidebar out over the terminal when it is hidden (auto-hide or
 *  collapsed); hides it again on a second press. */
export function peekSidebar() {
  if (!app.sidebarAutoHide && !app.sidebarCollapsed) return toggleSidebar();
  app.sidebarPeek = !app.sidebarPeek;
  if (!app.sidebarPeek) terminals.focusActive();
}

export function toggleAutoHide() {
  const on = !app.sidebarAutoHide;
  setSidebarAutoHide(on);
  // Auto-hide replaces collapsing; turning it off shows the sidebar docked.
  if (!on && app.sidebarCollapsed) setSidebar(false);
  toast(on ? `Sidebar auto-hides: point at the left edge or press ${app.config?.keys.peek_sidebar ?? "Ctrl+Alt+B"}` : "Sidebar stays open");
  setTimeout(() => terminals.fitActive(), 50);
}

export function dismissEnded(key: string) {
  terminals.dispose(key);
  const s = app.sessions.find((x) => x.key === key);
  if (s && !s.persistent) api.killSession(s.host, s.slug).catch(() => {});
}

/** Phone layout: the terminal full screen. Back (gesture or button) returns
 *  to the list through the history entry pushed here. */
export function showPhoneTerminal() {
  if (!app.phone || !app.phoneList) return;
  app.phoneList = false;
  history.pushState({ myterm: "term" }, "");
  setTimeout(() => {
    terminals.fitActive();
    terminals.focusActive();
  }, 50);
}

export function showPhoneList() {
  if (!app.phone || app.phoneList) return;
  if (history.state?.myterm === "term") history.back();
  else app.phoneList = true;
}
