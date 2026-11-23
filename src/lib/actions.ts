import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, type HostInfo, type Session, type Window } from "./api";
import { app, hostInfo, isReachable, setSidebar, toast, windowTarget, type Row } from "./store.svelte";
import { terminals } from "./terminals";

export function activate(s: Session) {
  app.overlay = null;
  app.menu = null;
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

export async function createSession(host: string, dir: string, name: string, kind: "claude" | "shell") {
  const h = hostInfo(host);
  if (!h) return toast(`Unknown host ${host}`, true);
  app.overlay = null;
  if (h.kind === "ephemeral") {
    await terminals.openEphemeral(host, dir, name, kind);
    return;
  }
  if (h.kind !== "tmux" || !h.connected) return toast(`${host} is not available: ${h.status}`, true);
  const target = windowTarget(host);
  if (target) {
    try {
      const id = await api.createWindow(host, target.slug, dir, name, kind);
      // The backend refreshes before returning; mark the new window current
      // in case that poll raced the switch.
      const s = app.sessions.find((x) => x.key === target.key) ?? target;
      for (const w of s.windows) w.active = w.id === id;
      terminals.open(s);
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
  } catch (e) {
    toast(String(e), true);
  }
}

export function hostAction(h: HostInfo) {
  if (h.needs_login) {
    terminals.openLogin(h.alias);
  } else {
    reconnectHost(h.alias);
  }
}

export async function reconnectHost(alias: string) {
  try {
    await api.reconnectHost(alias);
    toast(`Reconnecting ${alias}`);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function setHostHidden(alias: string, hidden: boolean) {
  try {
    await api.setHostHidden(alias, hidden);
    toast(hidden ? `Hid ${alias}` : `Showing ${alias}`);
  } catch (e) {
    toast(String(e), true);
  }
}

export async function openSettings() {
  try {
    await api.openConfig();
  } catch (e) {
    toast(String(e), true);
  }
}

/** Hosts come from ~/.ssh/config; adding one means editing it. */
export async function addHost() {
  try {
    await api.openSshConfig();
    toast("Add a Host block to your ssh config; myterm picks it up when you save");
  } catch (e) {
    toast(String(e), true);
  }
}

export function toggleSidebar() {
  setSidebar(!app.sidebarCollapsed);
  setTimeout(() => terminals.fitActive(), 50);
}

export function dismissEnded(key: string) {
  terminals.dispose(key);
  const s = app.sessions.find((x) => x.key === key);
  if (s && !s.persistent) api.killSession(s.host, s.slug).catch(() => {});
}
