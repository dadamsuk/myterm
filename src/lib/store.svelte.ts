import { isLive } from "./hostdot";
import type { Config, HostInfo, ListMode, SavedSession, Session, Window } from "./api";
import { api } from "./api";

export type Overlay =
  | { kind: "switcher" }
  | { kind: "palette" }
  | { kind: "new"; sessionKind?: "claude" | "shell"; host?: string }
  | { kind: "rename"; row: Row }
  | { kind: "hooks"; host: string; install: boolean }
  | { kind: "confirm-quit"; count: number }
  | { kind: "hosts"; host?: string }
  | { kind: "settings" }
  /** Save a running session (`row`) or edit a saved one (`saved`). */
  | { kind: "save"; row?: Row; saved?: SavedSession }
  | null;

export interface TermStatus {
  status: "connecting" | "live" | "reconnecting" | "ended" | "detached" | "error";
  message?: string;
}

export const app = $state({
  ready: false,
  platform: "windows" as "windows" | "macos" | "linux" | "android",
  config: null as Config | null,
  configError: null as string | null,
  hosts: [] as HostInfo[],
  sessions: [] as Session[],
  lastViewed: {} as Record<string, number>,
  knownFolders: {} as Record<string, Record<string, number>>,
  listMode: "recent" as ListMode,
  sidebarWidth: 280,
  sidebarCollapsed: false,
  /** The sidebar is hidden and slides over the terminal on demand. */
  sidebarAutoHide: false,
  /** The sidebar is showing over the terminal (auto-hide or collapsed). */
  sidebarPeek: false,
  /** Points added to the configured font size (Ctrl+= / Ctrl+-). */
  fontZoom: 0,
  /** Key of the terminal on screen: a session key, or "login:<host>". */
  active: null as string | null,
  termStatus: {} as Record<string, TermStatus>,
  overlay: null as Overlay,
  menu: null as { x: number; y: number; row: Row } | null,
  /** Right-click menu on a terminal. */
  termMenu: null as { x: number; y: number; key: string; selection: boolean } | null,
  toast: null as { text: string; error?: boolean } | null,
  /** Claude or plain shell: the last choice in the new-session form. */
  newKind: "claude" as "claude" | "shell",
  /** Phone layout (spec 14.4): narrower than 700 px. */
  phone: false,
  /** Phone layout: the list is on screen rather than the terminal. */
  phoneList: true,
  /** Keys row: Ctrl or Alt waiting to apply to the next key. */
  keyMods: { ctrl: false, alt: false },
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
  const listed = new Set(app.hosts.map((h) => h.alias));
  return app.sessions.filter((s) => listed.has(s.host));
}

export function lastUsed(s: Session): number {
  return Math.max(app.lastViewed[s.key] ?? 0, s.last_attached || 0, s.persistent ? 0 : s.created);
}

/** Key for a window's last-viewed time: "host/slug:@3". */
export function windowKey(s: Session, w: Window): string {
  return `${s.key}:${w.id}`;
}

export function activeWindow(s: Session): Window | undefined {
  return s.windows.find((w) => w.active);
}

/** One line in the list: a tmux window, or a whole session when it has no
 *  windows (ephemeral sessions). tmux sessions themselves are not shown. */
export interface Row {
  key: string;
  s: Session;
  w?: Window;
}

export function rowsOf(s: Session): Row[] {
  return s.windows.length ? s.windows.map((w) => ({ key: windowKey(s, w), s, w })) : [{ key: s.key, s }];
}

export function visibleRows(): Row[] {
  return visibleSessions().flatMap(rowsOf);
}

/** The row on screen: the active window of the session being shown. */
export function activeRow(): Row | undefined {
  const s = sessionByKey(app.active);
  if (!s) return undefined;
  return rowsOf(s).find(rowOnScreen);
}

export const rowName = (r: Row) => (r.w ? r.w.name : r.s.display_name);
export const rowCwd = (r: Row) => (r.w ? r.w.cwd : r.s.cwd);
export const rowState = (r: Row) => (r.w ? r.w.state : r.s.state);
const rowStateAt = (r: Row) => (r.w ? r.w.state_at : r.s.state_at);

export function rowOnScreen(r: Row): boolean {
  return app.active === r.s.key && (!r.w || r.w.active);
}

/** permission always; done until it has been on screen since it happened. */
export function rowNeedsAttention(r: Row): boolean {
  if (r.s.ended) return false;
  const st = rowState(r);
  if (st === "permission") return true;
  if (st === "done") return !rowOnScreen(r) && rowStateAt(r) > (app.lastViewed[r.key] ?? 0);
  return false;
}

export function rowDot(r: Row): string {
  if (r.s.ended) return "none";
  const st = rowState(r);
  if (st === "permission") return "permission";
  if (st === "done" && rowNeedsAttention(r)) return "done";
  if (st === "working") return "working";
  // Any other Claude window: a quiet ring, so Claude rows stand out from shells.
  return rowKind(r) === "claude" ? "idle" : "none";
}

const rowKind = (r: Row) => (r.w ? r.w.kind : r.s.kind);

/** Words for a row's status, for hover text. */
export function rowStatusText(r: Row): string {
  if (rowKind(r) !== "claude") return "";
  switch (rowDot(r)) {
    case "permission":
      return "Claude: waiting for permission";
    case "working":
      return "Claude: working";
    case "done":
      return "Claude: finished (not seen yet)";
    default:
      return "Claude: idle";
  }
}

/** A window's own last-viewed time; the current window also counts tmux's
 *  last-attached time for its session. */
export function rowLastUsed(r: Row): number {
  if (!r.w) return lastUsed(r.s);
  return Math.max(app.lastViewed[r.key] ?? 0, r.w.active ? lastUsed(r.s) : 0);
}

/** True when a host has more than one tmux session, so rows name their session. */
export function multiSession(host: string): boolean {
  if (hostInfo(host)?.kind !== "tmux") return false;
  return app.sessions.filter((s) => s.host === host && s.persistent).length > 1;
}

/** On a tmux host that already has sessions, new work becomes a window in
 *  the most recently used one. */
export function windowTarget(host: string): Session | undefined {
  const h = hostInfo(host);
  if (!h || h.kind !== "tmux") return undefined;
  return app.sessions
    .filter((s) => s.host === host && s.persistent && !s.ended)
    .sort((a, b) => lastUsed(b) - lastUsed(a))[0];
}

export function isReachable(s: Session): boolean {
  const h = hostInfo(s.host);
  if (!s.persistent) return !s.ended;
  return isLive(h);
}

export function basename(p: string): string {
  const t = p.replace(/[\\/]+$/, "");
  const i = Math.max(t.lastIndexOf("/"), t.lastIndexOf("\\"));
  return i >= 0 ? t.slice(i + 1) || t : t;
}

const cmp = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });
const byName = (a: Row, b: Row) => cmp(rowName(a), rowName(b));
/** tmux order: session by name, then window index, as in the status bar. */
const byTmuxOrder = (a: Row, b: Row) =>
  cmp(a.s.display_name, b.s.display_name) || (a.w?.index ?? 0) - (b.w?.index ?? 0);
const byAttention = (a: Row, b: Row) => rowStateAt(b) - rowStateAt(a);
const byRecent = (a: Row, b: Row) => rowLastUsed(b) - rowLastUsed(a) || byName(a, b);

export interface Group {
  title: string;
  /** Unique key for the list; defaults to title plus host. */
  key?: string;
  host?: HostInfo;
  /** A name group in Name mode: its rows are labelled by machine. */
  byName?: boolean;
  rows: Row[];
}

export function groupedRows(): Group[] {
  const all = visibleRows();
  const pinned = app.config?.pin_attention === true ? all.filter(rowNeedsAttention) : [];
  const pinnedKeys = new Set(pinned.map((r) => r.key));
  const rest = all.filter((r) => !pinnedKeys.has(r.key));
  const groups: Group[] = [];
  if (pinned.length) groups.push({ title: "Needs you", rows: pinned.sort(byAttention) });
  if (app.listMode === "machine") {
    for (const h of app.hosts) {
      groups.push({ title: h.alias, host: h, rows: rest.filter((r) => r.s.host === h.alias).sort(byTmuxOrder) });
    }
  } else if (app.listMode === "name") {
    // One group per name, its rows one per machine; a name used once sits at
    // the top level as an untitled group of one.
    const hostOrder = new Map(app.hosts.map((h, i) => [h.alias, i]));
    const byHost = (a: Row, b: Row) =>
      (hostOrder.get(a.s.host) ?? 1e9) - (hostOrder.get(b.s.host) ?? 1e9) || byTmuxOrder(a, b);
    const byNameKey = new Map<string, Row[]>();
    for (const r of rest.sort(byName)) {
      // Case-insensitive: "BigSkySim" and "bigskysim" share a group.
      const n = rowName(r).toLocaleLowerCase();
      const list = byNameKey.get(n);
      if (list) list.push(r);
      else byNameKey.set(n, [r]);
    }
    for (const [n, rows] of byNameKey) {
      if (rows.length === 1) groups.push({ title: "", key: `name:${rows[0].key}`, rows });
      else groups.push({ title: rowName(rows[0]), key: `name:${n}`, byName: true, rows: rows.sort(byHost) });
    }
  } else {
    groups.push({ title: "", rows: rest.sort(byRecent) });
  }
  return groups;
}

/** Rows in sidebar order; row i (0-based) is switched to with Ctrl+Shift+(i+1). */
export function sidebarRows(): Row[] {
  return groupedRows().flatMap((g) => g.rows);
}

/** Hover text for a row: name, then user@host and the folder. */
export function rowTooltip(r: Row): string {
  const user = app.hosts.find((h) => h.alias === r.s.host)?.user;
  const status = rowStatusText(r);
  const lines = [rowName(r), `${user ? user + "@" : ""}${r.s.host}`, rowCwd(r)];
  if (status) lines.push(status);
  return lines.join("\n");
}

/** Switcher order for an empty query: attention, then recent; current last. */
export function switcherOrder(): Row[] {
  const all = visibleRows();
  const att = all.filter(rowNeedsAttention).sort(byAttention);
  const attKeys = new Set(att.map((r) => r.key));
  const list = [...att, ...all.filter((r) => !attKeys.has(r.key)).sort(byRecent)];
  const i = list.findIndex(rowOnScreen);
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

/** Marks a session as seen now, and its current window. */
export function markViewed(key: string) {
  const now = Math.floor(Date.now() / 1000);
  const keys = [key];
  const s = sessionByKey(key);
  const w = s ? activeWindow(s) : undefined;
  if (s && w) keys.push(windowKey(s, w));
  for (const k of keys) {
    app.lastViewed[k] = now;
    api.markViewed(k).catch(() => {});
  }
}

export function setListMode(m: ListMode) {
  app.listMode = m;
  api.setUiState({ listMode: m }).catch(() => {});
}

export function setSidebarAutoHide(on: boolean) {
  app.sidebarAutoHide = on;
  app.sidebarPeek = false;
  api.setUiState({ sidebarAutoHide: on }).catch(() => {});
}

export function setSidebar(collapsed: boolean, width?: number) {
  app.sidebarCollapsed = collapsed;
  if (width !== undefined) app.sidebarWidth = width;
  api.setUiState({ sidebarCollapsed: collapsed, sidebarWidth: width }).catch(() => {});
}

/** Terminal font size: the configured size plus the zoom, kept readable. */
export function fontSize(): number {
  return Math.min(40, Math.max(6, (app.config?.font_size ?? 13) + app.fontZoom));
}

export interface FolderEntry {
  host: string;
  path: string;
  t: number;
}

/** Every folder seen across hosts; one entry per (host, path), newest first. */
export function knownFolders(): FolderEntry[] {
  const out: FolderEntry[] = [];
  const listed = new Set(app.hosts.map((h) => h.alias));
  for (const [host, m] of Object.entries(app.knownFolders)) {
    if (!listed.has(host)) continue;
    for (const [path, t] of Object.entries(m)) out.push({ host, path, t });
  }
  for (const s of app.sessions) {
    if (s.cwd && !out.some((f) => f.host === s.host && f.path === s.cwd) && listed.has(s.host))
      out.push({ host: s.host, path: s.cwd, t: lastUsed(s) });
  }
  return out.sort((a, b) => b.t - a.t);
}
