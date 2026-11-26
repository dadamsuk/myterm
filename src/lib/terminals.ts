import { Terminal, type ITheme } from "@xterm/xterm";
import { attachMobileInput } from "./mobileinput";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { ClipboardAddon, type IClipboardProvider } from "@xterm/addon-clipboard";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@xterm/xterm/css/xterm.css";
import { api, Channel, type Config, type PtyMessage, type Session } from "./api";
import { activeRow, app, fontSize, hostInfo, isMac, isReachable, markViewed, sessionByKey, toast } from "./store.svelte";
import { matches, parseBinding } from "./keys";
import { sessionKeyOfRow } from "./rowkey";

const BACKOFF = [1, 2, 5, 10, 30];

type Kind = "tmux" | "ephemeral" | "login";

interface Entry {
  key: string;
  kind: Kind;
  host: string;
  slug: string;
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
  pty: number | null;
  gen: number;
  closedByUs: boolean;
  attempt: number;
  retryTimer?: ReturnType<typeof setTimeout>;
  connectedAt: number;
  lastShown: number;
  pendingInput?: string;
  /** Input produced before the PTY id is known, e.g. xterm's reply to ConPTY's
   *  startup cursor query; dropping it stalls the start by ~3 s. */
  early: string[];
  ack: number;
  ackTimer?: ReturnType<typeof setTimeout>;
  /** When output last arrived, for waiting until a command settles. */
  lastOutput: number;
  /** Shell lines typed here, for "Save session…" (see `capture`). */
  typed: { lines: string[]; cur: string; edited: boolean; skipped: number };
}

/** How long start-up keeps waiting for the hosts of sessions to reopen. */
const RESTORE_MS = 60000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const clipboardProvider: IClipboardProvider = {
  // Programs may set the clipboard through OSC 52, but never read it.
  readText: async () => "",
  writeText: async (_sel, text) => {
    await writeText(text);
  },
};

/** Quotes a path for typing into a shell or Claude Code prompt, only when needed. */
export function quotePath(p: string): string {
  if (/^[\w@%+=:,./\\~-]+$/.test(p)) return p;
  if (/^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\")) return `"${p}"`;
  return `'${p.replace(/'/g, "'\\''")}'`;
}

function themeOf(cfg: Config | null): ITheme {
  return (cfg?.theme ?? {}) as ITheme;
}

class TerminalManager {
  private entries = new Map<string, Entry>();
  private container: HTMLElement | null = null;
  private resizeTimer?: ReturnType<typeof setTimeout>;
  private seq = 0;
  private viewTimer?: ReturnType<typeof setTimeout>;
  /** Sessions saved as open, still waiting for their host at start-up. */
  private restoring: { keys: string[]; active: string | null; until: number } | null = null;

  mount(container: HTMLElement) {
    this.container = container;
    for (const e of this.entries.values()) container.appendChild(e.el);
    new ResizeObserver(() => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = setTimeout(() => this.fitActive(), 50);
    }).observe(container);
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        if (ev.payload.type !== "drop" || !ev.payload.paths.length) return;
        const e = app.active ? this.entries.get(app.active) : undefined;
        if (e) this.dropFiles(e, ev.payload.paths);
      })
      .catch(() => {});
  }

  /** Types the paths of dropped files, uploading them first for remote hosts. */
  private async dropFiles(e: Entry, paths: string[]) {
    const remote = !hostInfo(e.host)?.local;
    if (remote) toast(`Uploading ${paths.length === 1 ? "file" : `${paths.length} files`} to ${e.host}…`);
    try {
      const out = await api.dropFiles(e.host, paths);
      if (remote) app.toast = null;
      e.term.paste(out.map(quotePath).join(" "));
      e.term.focus();
    } catch (err) {
      toast(String(err), true);
    }
  }

  /** Pastes clipboard text; failing that, a clipboard image saved as a file on the host. */
  private async pasteInto(e: Entry) {
    const t = await readText().catch(() => "");
    if (t) {
      e.term.paste(t);
      return;
    }
    const remote = !hostInfo(e.host)?.local;
    let shown = false;
    const timer = remote ? setTimeout(() => ((shown = true), toast(`Uploading image to ${e.host}…`)), 300) : undefined;
    try {
      const path = await api.pasteImage(e.host);
      if (shown) app.toast = null;
      if (path) e.term.paste(quotePath(path));
    } catch (err) {
      toast(String(err), true);
    } finally {
      clearTimeout(timer);
    }
  }

  has(key: string) {
    return this.entries.has(key);
  }

  keys() {
    return [...this.entries.keys()];
  }

  /** `scroll`: keep scrollback here. tmux keeps its own; a keeper's ConPTY keeps none. */
  private create(key: string, kind: Kind, host: string, slug: string, scroll = kind !== "tmux"): Entry {
    const cfg = app.config;
    const term = new Terminal({
      allowProposedApi: true,
      fontFamily: cfg?.font_family,
      fontSize: fontSize(),
      lineHeight: cfg?.line_height ?? 1.1,
      theme: themeOf(cfg),
      cursorBlink: false,
      // tmux owns scrollback for tmux sessions.
      scrollback: scroll ? 10000 : 0,
      macOptionClickForcesSelection: true,
      // Shift+drag does a native selection while tmux has the mouse.
      altClickMovesCursor: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const el = document.createElement("div");
    el.className = "term" + (scroll ? "" : " no-scrollbar");
    el.style.display = "none";
    this.container?.appendChild(el);
    term.open(el);
    const unicode = new Unicode11Addon();
    term.loadAddon(unicode);
    term.unicode.activeVersion = "11";
    term.loadAddon(new WebLinksAddon((ev, uri) => {
      ev.preventDefault();
      openUrl(uri).catch(() => {});
    }));
    term.loadAddon(new ClipboardAddon(undefined, clipboardProvider));
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose()); // falls back to the DOM renderer
      term.loadAddon(webgl);
    } catch {
      /* DOM renderer */
    }
    const e: Entry = {
      key,
      kind,
      host,
      slug,
      term,
      fit,
      el,
      pty: null,
      gen: 0,
      closedByUs: false,
      attempt: 0,
      connectedAt: 0,
      lastShown: Date.now(),
      early: [],
      ack: 0,
      lastOutput: 0,
      typed: { lines: [], cur: "", edited: false, skipped: 0 },
    };
    term.onData((d) => {
      this.capture(e, d);
      this.send(e, d);
    });
    term.onResize(({ cols, rows }) => {
      if (e.pty !== null) api.resize(e.pty, cols, rows).catch(() => {});
    });
    term.attachCustomKeyEventHandler((ev) => this.keyHandler(e, ev));
    if (app.platform === "android" && term.textarea) {
      attachMobileInput(el, term.textarea, (d) => {
        this.capture(e, d);
        this.send(e, d);
      });
    }
    // Right-click opens myterm's menu rather than reaching tmux's own.
    el.addEventListener(
      "mousedown",
      (ev) => {
        if (ev.button === 2) ev.stopPropagation();
      },
      true,
    );
    el.addEventListener(
      "mouseup",
      (ev) => {
        if (ev.button === 2) ev.stopPropagation();
      },
      true,
    );
    el.addEventListener(
      "contextmenu",
      (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        app.termMenu = { x: ev.clientX, y: ev.clientY, key: e.key, selection: term.hasSelection() };
      },
      true,
    );
    this.entries.set(key, e);
    return e;
  }

  private keyHandler(e: Entry, ev: KeyboardEvent): boolean {
    const mac = isMac();
    const keys = app.config?.keys;
    if (ev.key === "Enter" && ev.shiftKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
      if (ev.type === "keydown") this.send(e, "\x1b\r");
      ev.preventDefault();
      return false;
    }
    const isCopy =
      (keys && matches(ev, parseBinding(keys.copy), false)) ||
      (mac && ev.metaKey && !ev.shiftKey && !ev.ctrlKey && ev.code === "KeyC");
    const isPaste =
      (keys && matches(ev, parseBinding(keys.paste), false)) ||
      (mac && ev.metaKey && !ev.shiftKey && !ev.ctrlKey && ev.code === "KeyV");
    if (isCopy) {
      if (ev.type === "keydown" && e.term.hasSelection()) {
        writeText(e.term.getSelection()).catch(() => {});
      }
      ev.preventDefault();
      return false;
    }
    if (isPaste) {
      if (ev.type === "keydown") this.pasteInto(e);
      ev.preventDefault();
      return false;
    }
    return true;
  }

  /**
   * Keeps the lines typed at a shell prompt, so "Save session…" can offer them.
   * Only plain typing and Backspace are followed: a line that used history,
   * Tab completion or cursor keys is not known for certain, so it is counted
   * in `skipped` and left out. Nothing is kept while a full-screen program
   * (an editor, Claude) has the alternate screen.
   */
  private capture(e: Entry, d: string) {
    if (e.kind === "login" || e.term.buffer.active.type === "alternate") return;
    const t = e.typed;
    for (const ch of d.replace(/\x1b\[20[01]~/g, "")) {
      if (ch === "\r" || ch === "\n") {
        if (t.edited) t.skipped++;
        else if (t.cur.trim() && t.lines.length < 50) t.lines.push(t.cur.trim());
        t.cur = "";
        t.edited = false;
      } else if (ch === "\x7f" || ch === "\b") {
        t.cur = [...t.cur].slice(0, -1).join("");
      } else if (ch === "\x03" || ch === "\x15") {
        // Ctrl+C, Ctrl+U: the line is abandoned.
        t.cur = "";
        t.edited = false;
      } else if (ch < " ") {
        t.edited = true;
      } else {
        t.cur += ch;
      }
    }
  }

  /** The shell lines typed in a terminal so far, and how many were left out. */
  typedLines(key: string): { lines: string[]; skipped: number } {
    const t = this.entries.get(key)?.typed;
    return t ? { lines: [...t.lines], skipped: t.skipped } : { lines: [], skipped: 0 };
  }

  /**
   * Types each line into a session, waiting for the shell to start and then
   * for each command's output to settle before the next. Anything typed early
   * waits in the console's input buffer, so `wsl` followed by a command for
   * the Linux shell works even when WSL is slow to start.
   */
  async runLines(key: string, lines: string[]) {
    const settle = async (min: number) => {
      const start = Date.now();
      for (;;) {
        await sleep(100);
        const e = this.entries.get(key);
        if (!e) return false;
        const now = Date.now();
        if (now - start > 30000) return true;
        if (e.pty !== null && !e.pendingInput && e.lastOutput && now - start >= min && now - e.lastOutput >= 800)
          return true;
      }
    };
    if (!(await settle(0))) return;
    for (let i = 0; i < lines.length; i++) {
      const e = this.entries.get(key);
      if (!e) return;
      this.send(e, lines[i] + "\r");
      if (i < lines.length - 1 && !(await settle(1500))) return;
    }
  }

  private send(e: Entry, data: string) {
    if (e.pty !== null) api.write(e.pty, data).catch(() => {});
    else if (e.early.length < 64) e.early.push(data);
  }

  /** Records the PTY id and sends whatever was typed or answered before it. */
  private setPty(e: Entry, id: number) {
    e.pty = id;
    const early = e.early.splice(0);
    if (early.length) api.write(id, early.join("")).catch(() => {});
  }

  private setStatus(key: string, status: (typeof app.termStatus)[string]["status"], message?: string) {
    app.termStatus[key] = { status, message };
  }

  private flushAck(e: Entry, id: number) {
    if (e.ackTimer) return;
    e.ackTimer = setTimeout(() => {
      e.ackTimer = undefined;
      const n = e.ack;
      e.ack = 0;
      if (n > 0) api.ack(id, n).catch(() => {});
    }, 16);
  }

  /** A channel bound to one PTY generation; stale messages are dropped. */
  private channel(e: Entry): Channel<PtyMessage> {
    const gen = ++e.gen;
    const chan = new Channel<PtyMessage>();
    let first = true;
    chan.onmessage = (msg) => {
      if (gen !== e.gen) return;
      if (msg instanceof ArrayBuffer || ArrayBuffer.isView(msg)) {
        const bytes = msg instanceof ArrayBuffer ? new Uint8Array(msg) : new Uint8Array((msg as ArrayBufferView).buffer);
        const id = e.pty;
        e.lastOutput = Date.now();
        e.term.write(bytes, () => {
          if (id === null) return;
          e.ack += bytes.length;
          this.flushAck(e, id);
        });
        if (first) {
          first = false;
          if (app.termStatus[e.key]?.status !== "live") this.setStatus(e.key, "live");
          if (e.pendingInput) {
            const input = e.pendingInput;
            e.pendingInput = undefined;
            setTimeout(() => this.send(e, input), 400);
          }
        }
      } else if (msg && typeof msg === "object" && "exit" in msg) {
        this.onExit(e, (msg as { exit: number }).exit);
      }
    };
    return chan;
  }

  private async connect(e: Entry) {
    clearTimeout(e.retryTimer);
    if (e.pty !== null) {
      e.closedByUs = true;
      api.close(e.pty).catch(() => {});
      e.pty = null;
    }
    e.closedByUs = false;
    if (!app.termStatus[e.key] || app.termStatus[e.key].status !== "reconnecting") {
      this.setStatus(e.key, "connecting");
    }
    this.fitEntry(e);
    const chan = this.channel(e);
    e.early = [];
    try {
      e.connectedAt = Date.now();
      const gen = e.gen;
      const id =
        e.kind === "login"
          ? await api.openLogin(e.host, e.term.cols, e.term.rows, chan)
          : await api.attach(e.host, e.slug, e.term.cols, e.term.rows, chan);
      if (gen === e.gen) this.setPty(e, id);
      else api.close(id).catch(() => {}); // superseded by a newer attempt
    } catch (err) {
      if (!this.entries.has(e.key)) return;
      this.scheduleReconnect(e, String(err));
    }
  }

  private scheduleReconnect(e: Entry, why?: string) {
    const d = BACKOFF[Math.min(e.attempt, BACKOFF.length - 1)];
    e.attempt++;
    this.setStatus(e.key, "reconnecting", `${why ? why + " — " : ""}retrying in ${d}s`);
    clearTimeout(e.retryTimer);
    e.retryTimer = setTimeout(() => {
      if (this.entries.has(e.key)) this.connect(e);
    }, d * 1000);
  }

  private onExit(e: Entry, code: number) {
    if (e.closedByUs) return;
    e.pty = null;
    if (e.kind === "login") {
      this.setStatus(e.key, "ended", "Login session ended. The host is reconnecting.");
      api.reconnectHost(e.host).catch(() => {});
      return;
    }
    if (e.kind === "ephemeral") {
      this.setStatus(e.key, "ended", `Session ended (exit ${code})`);
      return;
    }
    const lived = Date.now() - e.connectedAt;
    if (lived > 15000) e.attempt = 0;
    if (code === 0) {
      // Either detached or the session ended; the next poll tells which.
      setTimeout(() => {
        if (!this.entries.has(e.key) || e.pty !== null) return;
        if (sessionByKey(e.key)) this.setStatus(e.key, "detached", "Detached");
        else this.setStatus(e.key, "ended", "Session ended");
      }, 1500);
      return;
    }
    const h = hostInfo(e.host);
    if (h?.connected && !sessionByKey(e.key) && lived > 0) {
      this.setStatus(e.key, "ended", "Session no longer exists");
      return;
    }
    this.scheduleReconnect(e, "connection lost");
  }

  /** Shows a session, attaching it first if needed. */
  open(s: Session) {
    let e = this.entries.get(s.key);
    if (!e) {
      if (!s.persistent) {
        toast("This ephemeral session belongs to a previous window", true);
        return;
      }
      e = this.create(s.key, "tmux", s.host, s.slug, hostInfo(s.host)?.kind === "keeper");
      this.show(s.key);
      this.connect(e);
    } else {
      this.show(s.key);
    }
    this.evict();
    this.viewChanged();
  }

  /** Attaches a session without showing it. The terminal is sized to the
   *  pane first, so tmux does not see an 80x24 client. */
  private openHidden(s: Session, lastShown: number) {
    if (this.entries.has(s.key)) return;
    const e = this.create(s.key, "tmux", s.host, s.slug, hostInfo(s.host)?.kind === "keeper");
    e.lastShown = lastShown;
    e.el.style.visibility = "hidden";
    e.el.style.display = "block";
    try {
      e.fit.fit();
    } catch {
      /* not laid out yet */
    }
    e.el.style.display = "none";
    e.el.style.visibility = "";
    this.connect(e);
  }

  /** Starts reopening the sessions that were open when the app last ran. */
  startRestore(keys: string[], active: string | null) {
    if (keys.length) this.restoring = { keys: [...keys], active: active ? sessionKeyOfRow(active) : null, until: Date.now() + RESTORE_MS };
  }

  /**
   * Opens each saved session once its host is usable, keeping their order of
   * use. The one that was on screen is shown, unless something else has been
   * opened by then. Its window is not selected: another client may have moved
   * on since, and tmux's current window is shared. Called when sessions or
   * hosts change.
   */
  restoreTick() {
    const r = this.restoring;
    if (!r) return;
    const now = Date.now();
    r.keys = r.keys.filter((key, i) => {
      const s = sessionByKey(key);
      if (!s || !s.persistent || !isReachable(s)) return true;
      if (key === r.active && app.active === null) this.open(s);
      else this.openHidden(s, now - (i + 1) * 1000);
      return false;
    });
    if (!r.keys.length || now > r.until) this.restoring = null;
    this.evict();
    this.viewChanged();
  }

  /** Saves which sessions are open and which row is on screen, shortly. */
  viewChanged() {
    clearTimeout(this.viewTimer);
    this.viewTimer = setTimeout(() => {
      const open = [...this.entries.values()]
        // An ended session has nothing to reopen.
        .filter((e) => e.kind === "tmux" && sessionByKey(e.key))
        .sort((a, b) => b.lastShown - a.lastShown)
        .map((e) => e.key);
      // Sessions still waiting for their host stay on the list, in case the
      // app is closed before they come back.
      for (const k of this.restoring?.keys ?? []) if (!open.includes(k)) open.push(k);
      const row = activeRow()?.key ?? (this.restoring?.active || "");
      api.setUiState({ openSessions: open, activeRow: row }).catch(() => {});
    }, 500);
  }

  openLogin(host: string) {
    const key = `login:${host}`;
    let e = this.entries.get(key);
    if (e) {
      this.show(key);
      if (e.pty === null) this.connect(e);
      return;
    }
    e = this.create(key, "login", host, "");
    this.show(key);
    this.connect(e);
  }

  /** Starts an ephemeral session; returns its key, or nothing if it failed. */
  async openEphemeral(host: string, dir: string, name: string, kind: string): Promise<string | undefined> {
    const tmp = `pending:${++this.seq}`;
    const e = this.create(tmp, "ephemeral", host, "");
    this.show(tmp);
    this.fitEntry(e);
    this.setStatus(tmp, "connecting");
    const chan = this.channel(e);
    try {
      const r = await api.startEphemeral(host, dir, name, kind, e.term.cols, e.term.rows, chan);
      this.setPty(e, r.pty);
      e.pendingInput = r.input || undefined;
      this.entries.delete(tmp);
      delete app.termStatus[tmp];
      e.key = r.key;
      e.slug = r.key.slice(host.length + 1);
      this.entries.set(r.key, e);
      this.setStatus(r.key, "live");
      if (app.active === tmp) this.show(r.key);
      return r.key;
    } catch (err) {
      this.dispose(tmp);
      toast(String(err), true);
    }
  }

  reattach(key: string) {
    const e = this.entries.get(key);
    if (!e) return;
    e.attempt = 0;
    this.connect(e);
  }

  show(key: string) {
    const prev = app.active;
    if (prev && prev !== key && sessionByKey(prev)) markViewed(prev);
    for (const [k, e] of this.entries) e.el.style.display = k === key ? "block" : "none";
    app.active = key;
    const e = this.entries.get(key);
    if (e) {
      e.lastShown = Date.now();
      requestAnimationFrame(() => {
        this.fitEntry(e);
        e.term.focus();
      });
    }
    if (sessionByKey(key)) markViewed(key);
    this.viewChanged();
  }

  /** Menu actions; each returns focus to the terminal. */
  async copy(key: string) {
    const e = this.entries.get(key);
    if (e?.term.hasSelection()) await writeText(e.term.getSelection()).catch(() => {});
    e?.term.focus();
  }

  async paste(key: string) {
    const e = this.entries.get(key);
    if (!e) return;
    await this.pasteInto(e);
    e.term.focus();
  }

  selectAll(key: string) {
    const e = this.entries.get(key);
    e?.term.selectAll();
    e?.term.focus();
  }

  /** Clears local scrollback; tmux sessions keep theirs on the server. */
  clear(key: string) {
    const e = this.entries.get(key);
    e?.term.clear();
    e?.term.focus();
  }

  /**
   * Turns off input modes a program left on when it exited or was killed:
   * mouse reporting, focus events, bracketed paste, application cursor keys and
   * keypad, and a hidden cursor. Stops echoed junk like `^[[<35;41;23M` on mouse
   * moves. Only myterm's emulator changes; nothing is sent to the program, and
   * the screen and scrollback are kept.
   */
  resetModes(key: string) {
    const e = this.entries.get(key);
    if (!e) return;
    e.term.write(
      "\x1b[?9l\x1b[?1000l\x1b[?1001l\x1b[?1002l\x1b[?1003l" + // mouse tracking
        "\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l" + // mouse encodings
        "\x1b[?1004l\x1b[?2004l" + // focus events, bracketed paste
        "\x1b[?1l\x1b>" + // normal cursor keys and keypad
        "\x1b[?25h", // show the cursor
    );
    e.term.focus();
  }

  isTmux(key: string) {
    return this.entries.get(key)?.kind === "tmux";
  }

  focusActive() {
    const e = app.active ? this.entries.get(app.active) : undefined;
    e?.term.focus();
  }

  private fitEntry(e: Entry) {
    if (e.el.style.display === "none" || !e.el.isConnected) return;
    try {
      e.fit.fit();
    } catch {
      /* not laid out yet */
    }
  }

  fitActive() {
    const e = app.active ? this.entries.get(app.active) : undefined;
    if (e) this.fitEntry(e);
  }

  dispose(key: string) {
    const e = this.entries.get(key);
    if (!e) return;
    clearTimeout(e.retryTimer);
    e.closedByUs = true;
    e.gen++;
    if (e.pty !== null) api.close(e.pty).catch(() => {});
    e.term.dispose();
    e.el.remove();
    this.entries.delete(key);
    delete app.termStatus[key];
    if (app.active === key) app.active = null;
    this.viewChanged();
  }

  /** Detaches the least recently viewed tmux sessions beyond the cap. */
  private evict() {
    const cap = Math.max(1, app.config?.max_open_sessions ?? 8);
    const candidates = [...this.entries.values()]
      .filter((e) => e.kind === "tmux" && e.key !== app.active)
      .sort((a, b) => a.lastShown - b.lastShown);
    let open = this.entries.size;
    for (const e of candidates) {
      if (open <= cap) break;
      this.dispose(e.key);
      open--;
    }
  }

  /** Changes the font size of every terminal; each refits and tells its PTY. */
  zoom(delta: number) {
    const before = fontSize();
    app.fontZoom = delta === 0 ? 0 : app.fontZoom + delta;
    // Keep the stored offset inside the clamp, so zooming back responds at once.
    app.fontZoom = fontSize() - (app.config?.font_size ?? 13);
    api.setUiState({ fontZoom: app.fontZoom }).catch(() => {});
    const size = fontSize();
    if (size === before) return;
    for (const e of this.entries.values()) e.term.options.fontSize = size;
    for (const e of this.entries.values()) this.fitEntry(e);
    toast(`Font size ${size}${app.fontZoom ? "" : " (default)"}`);
  }

  applyConfig(cfg: Config) {
    for (const e of this.entries.values()) {
      e.term.options.fontFamily = cfg.font_family;
      e.term.options.fontSize = fontSize();
      e.term.options.lineHeight = cfg.line_height;
      e.term.options.theme = themeOf(cfg);
    }
    this.fitActive();
  }

  runningEphemerals(): number {
    return [...this.entries.values()].filter((e) => e.kind === "ephemeral" && e.pty !== null).length;
  }
}

export const terminals = new TerminalManager();
