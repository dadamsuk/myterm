import { invoke, Channel } from "@tauri-apps/api/core";

export interface Keys {
  switcher: string;
  palette: string;
  new_session: string;
  copy: string;
  paste: string;
  toggle_sidebar: string;
}

export interface Config {
  font_family: string;
  font_size: number;
  line_height: number;
  theme: Record<string, string>;
  keys: Keys;
  hidden_hosts: string[];
  local_alias: string | null;
  hide_tmux_status: boolean;
  max_open_sessions: number;
  pin_attention: boolean;
  notify_permission: boolean;
  mouse: boolean;
  local_shell: string | null;
}

export interface HostInfo {
  alias: string;
  kind: "tmux" | "ephemeral" | "unavailable";
  connected: boolean;
  connecting: boolean;
  status: string;
  needs_login: boolean;
  os: string;
  tmux: string | null;
  tmux_version: string | null;
  home: string | null;
  hidden: boolean;
  local: boolean;
  win_powershell: boolean;
  hooks_installed: boolean;
}

export interface Session {
  key: string;
  host: string;
  slug: string;
  display_name: string;
  kind: "claude" | "shell";
  cwd: string;
  state: "working" | "permission" | "done" | null;
  state_at: number;
  created: number;
  last_attached: number;
  attached: number;
  persistent: boolean;
  ephemeral_id: number | null;
  ended: boolean;
}

export interface Snapshot {
  hosts: HostInfo[];
  sessions: Session[];
}

export interface UiState {
  last_viewed: Record<string, number>;
  known_folders: Record<string, Record<string, number>>;
  list_mode: ListMode;
  sidebar_width: number;
  sidebar_collapsed: boolean;
}

export type ListMode = "machine" | "name" | "recent";

export interface Boot {
  config: Config;
  config_error: string | null;
  state: UiState;
  snapshot: Snapshot;
  platform: "windows" | "macos" | "linux";
}

export interface FileChange {
  path: string;
  before: string;
  after: string;
}

export interface HookPlan {
  host: string;
  install: boolean;
  files: FileChange[];
}

export type PtyMessage = ArrayBuffer | { exit: number };

export const api = {
  boot: () => invoke<Boot>("boot"),
  attach: (host: string, slug: string, cols: number, rows: number, channel: Channel<PtyMessage>) =>
    invoke<number>("attach", { host, slug, cols, rows, channel }),
  openLogin: (host: string, cols: number, rows: number, channel: Channel<PtyMessage>) =>
    invoke<number>("open_login", { host, cols, rows, channel }),
  startEphemeral: (
    host: string,
    dir: string,
    name: string,
    kind: string,
    cols: number,
    rows: number,
    channel: Channel<PtyMessage>,
  ) => invoke<{ key: string; pty: number; input: string }>("start_ephemeral", { host, dir, name, kind, cols, rows, channel }),
  write: (id: number, data: string) => invoke<void>("pty_write", { id, data }),
  resize: (id: number, cols: number, rows: number) => invoke<void>("pty_resize", { id, cols, rows }),
  ack: (id: number, bytes: number) => invoke<void>("pty_ack", { id, bytes }),
  close: (id: number) => invoke<void>("pty_close", { id }),
  createSession: (host: string, dir: string, name: string, kind: string) =>
    invoke<string>("create_session", { host, dir, name, kind }),
  renameSession: (host: string, slug: string, name: string) => invoke<void>("rename_session", { host, slug, name }),
  killSession: (host: string, slug: string) => invoke<void>("kill_session", { host, slug }),
  completePath: (host: string, prefix: string) => invoke<string[]>("complete_path", { host, prefix }),
  attachCommandText: (host: string, slug: string) => invoke<string>("attach_command_text", { host, slug }),
  reconnectHost: (host: string) => invoke<void>("reconnect_host", { host }),
  setHostHidden: (host: string, hidden: boolean) => invoke<void>("set_host_hidden", { host, hidden }),
  hooksPlan: (host: string, install: boolean) => invoke<HookPlan>("hooks_plan", { host, install }),
  hooksApply: (host: string, install: boolean) => invoke<void>("hooks_apply", { host, install }),
  markViewed: (key: string) => invoke<void>("mark_viewed", { key }),
  setUiState: (s: { listMode?: ListMode; sidebarWidth?: number; sidebarCollapsed?: boolean }) =>
    invoke<void>("set_ui_state", s),
  getState: () => invoke<UiState>("get_state"),
  configPath: () => invoke<string>("config_path"),
  quit: () => invoke<void>("quit"),
};

export { Channel };
