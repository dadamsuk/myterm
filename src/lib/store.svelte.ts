import type { Config, HostInfo, ListMode, Session } from "./api";
import { api } from "./api";

export type Overlay =
  | { kind: "switcher" }
  | { kind: "palette" }
  | { kind: "new"; sessionKind: "claude" | "shell" }
  | { kind: "rename"; session: Session }
  | { kind: "hooks"; host: string; install: boolean }
  | { kind: "confirm-quit"; count: number }
  | null;

export interface TermStatus {
  status: "connecting" | "live" | "reconnecting" | "ended" | "detached" | "error";
  message?: string;
}

export const app = $state({
  ready: false,
  platform: "windows" as "windows" | "macos" | "linux",
  config: null as Config | null,
  configError: null as string | null,
  hosts: [] as HostInfo[],
  sessions: [] as Session[],
  lastViewed: {} as Record<string, number>,
  knownFolders: {} as Record<string, Record<string, number>>,
  listMode: "recent" as ListMode,
  sidebarWidth: 280,
  sidebarCollapsed: false,
  /** Key of the terminal on screen: a session key, or "login:<host>". */
  active: null as string | null,
  termStatus: {} as Record<string, TermStatus>,
  overlay: null as Overlay,
  menu: null as { x: number; y: number; session: Session } | null,
  toast: null as { text: string; error?: boolean } | null,
});

export const isMac = () => app.platform === "macos";

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string, error = false) {
  app.toast = { text, error };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = null), error ? 6000 : 2500);
}

export function hostInfo(alias: string): HostInfo | undefined {
  return app.hosts.find((h) => h.alias === alias);
}

export function sessionByKey(key: string | null): Session | undefined {
  return key ? app.sessions.find((s) => s.key === key) : undefined;
}

export function visibleSessions(): Session[] {
  const hidden = new Set(app.hosts.filter((h) => h.hidden).map((h) => h.alias));
  return app.sessions.filter((s) => !hidden.has(s.host));
}

export function lastUsed(s: Session): number {
  return Math.max(app.lastViewed[s.key] ?? 0, s.last_attached || 0, s.persistent ? 0 : s.created);
}

/** permission always; done until it has been on screen since it happened. */
export function needsAttention(s: Session): boolean {
  if (s.ended) return false;
  if (s.state === "permission") return true;
  if (s.state === "done") return s.key !== app.active && s.state_at > (app.lastViewed[s.key] ?? 0);
  return false;
}

export function dotClass(s: Session): string {
  if (s.ended) return "none";
  if (s.state === "working") return "working";
  if (s.state === "permission") return "permission";
  if (s.state === "done" && needsAttention(s)) return "done";
  return "none";
}

export function isReachable(s: Session): boolean {
  const h = hostInfo(s.host);
  if (!s.persistent) return !s.ended;
  return !!h && h.kind === "tmux" && h.connected;
}

export function basename(p: string): string {
  const t = p.replace(/[\\/]+$/, "");
  const i = Math.max(t.lastIndexOf("/"), t.lastIndexOf("\\"));
  return i >= 0 ? t.slice(i + 1) || t : t;
}

const byName = (a: Session, b: Session) =>
  a.display_name.localeCompare(b.display_name, undefined, { sensitivity: "base", numeric: true });

export interface Group {
  title: string;
  host?: HostInfo;
  sessions: Session[];
}

export function groupedSessions(): Group[] {
  const all = visibleSessions();
  const pinned = app.config?.pin_attention !== false ? all.filter(needsAttention) : [];
  const pinnedKeys = new Set(pinned.map((s) => s.key));
  const rest = all.filter((s) => !pinnedKeys.has(s.key));
  const groups: Group[] = [];
  if (pinned.length) {
    groups.push({ title: "Needs you", sessions: pinned.sort((a, b) => b.state_at - a.state_at) });
  }
  if (app.listMode === "machine") {
    for (const h of app.hosts) {
      if (h.hidden) continue;
      const ss = rest.filter((s) => s.host === h.alias).sort(byName);
      groups.push({ title: h.alias, host: h, sessions: ss });
    }
  } else if (app.listMode === "name") {
    groups.push({ title: "", sessions: rest.sort(byName) });
  } else {
    groups.push({ title: "", sessions: rest.sort((a, b) => lastUsed(b) - lastUsed(a) || byName(a, b)) });
  }
  return groups;
}

/** Switcher order for an empty query: attention, then recent; current last. */
export function switcherOrder(): Session[] {
  const all = visibleSessions();
  const att = all.filter(needsAttention).sort((a, b) => b.state_at - a.state_at);
  const attKeys = new Set(att.map((s) => s.key));
  const recent = all.filter((s) => !attKeys.has(s.key)).sort((a, b) => lastUsed(b) - lastUsed(a) || byName(a, b));
  const list = [...att, ...recent];
  const i = list.findIndex((s) => s.key === app.active);
  if (i >= 0) list.push(list.splice(i, 1)[0]);
  return list;
}

export function relTime(t: number): string {
  if (!t) return "";
  const d = Math.max(0, Date.now() / 1000 - t);
  if (d < 60) return "now";
  if (d < 3600) return `${Math.floor(d / 60)}m`;
  if (d < 86400) return `${Math.floor(d / 3600)}h`;
  if (d < 86400 * 30) return `${Math.floor(d / 86400)}d`;
  return `${Math.floor(d / (86400 * 30))}mo`;
}

export function markViewed(key: string) {
  app.lastViewed[key] = Math.floor(Date.now() / 1000);
  api.markViewed(key).catch(() => {});
}

export function setListMode(m: ListMode) {
  app.listMode = m;
  api.setUiState({ listMode: m }).catch(() => {});
}

export function setSidebar(collapsed: boolean, width?: number) {
  app.sidebarCollapsed = collapsed;
  if (width !== undefined) app.sidebarWidth = width;
  api.setUiState({ sidebarCollapsed: collapsed, sidebarWidth: width }).catch(() => {});
}

export interface FolderEntry {
  host: string;
  path: string;
  t: number;
}

/** Every folder seen across hosts; one entry per (host, path), newest first. */
export function knownFolders(): FolderEntry[] {
  const out: FolderEntry[] = [];
  const hidden = new Set(app.hosts.filter((h) => h.hidden).map((h) => h.alias));
  for (const [host, m] of Object.entries(app.knownFolders)) {
    if (hidden.has(host) || !app.hosts.some((h) => h.alias === host)) continue;
    for (const [path, t] of Object.entries(m)) out.push({ host, path, t });
  }
  for (const s of app.sessions) {
    if (s.cwd && !out.some((f) => f.host === s.host && f.path === s.cwd) && !hidden.has(s.host))
      out.push({ host: s.host, path: s.cwd, t: lastUsed(s) });
  }
  return out.sort((a, b) => b.t - a.t);
}
