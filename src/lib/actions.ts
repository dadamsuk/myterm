import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openPath } from "@tauri-apps/plugin-opener";
import { api, type HostInfo, type Session } from "./api";
import { app, hostInfo, isReachable, setSidebar, toast } from "./store.svelte";
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
    await openPath(await api.configPath());
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
