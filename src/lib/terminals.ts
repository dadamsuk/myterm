import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { ClipboardAddon, type IClipboardProvider } from "@xterm/addon-clipboard";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { api, Channel, type Config, type PtyMessage, type Session } from "./api";
import { app, hostInfo, isMac, markViewed, sessionByKey, toast } from "./store.svelte";
import { matches, parseBinding } from "./keys";

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
  ack: number;
  ackTimer?: ReturnType<typeof setTimeout>;
}

const clipboardProvider: IClipboardProvider = {
  // Programs may set the clipboard through OSC 52, but never read it.
  readText: async () => "",
  writeText: async (_sel, text) => {
    await writeText(text);
  },
};

function themeOf(cfg: Config | null): ITheme {
  return (cfg?.theme ?? {}) as ITheme;
}

class TerminalManager {
  private entries = new Map<string, Entry>();
  private container: HTMLElement | null = null;
  private resizeTimer?: ReturnType<typeof setTimeout>;
  private seq = 0;

  mount(container: HTMLElement) {
    this.container = container;
    for (const e of this.entries.values()) container.appendChild(e.el);
    new ResizeObserver(() => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = setTimeout(() => this.fitActive(), 50);
    }).observe(container);
  }

  has(key: string) {
    return this.entries.has(key);
  }

  keys() {
    return [...this.entries.keys()];
  }

  private create(key: string, kind: Kind, host: string, slug: string): Entry {
    const cfg = app.config;
    const term = new Terminal({
      allowProposedApi: true,
      fontFamily: cfg?.font_family,
      fontSize: cfg?.font_size ?? 13,
      lineHeight: cfg?.line_height ?? 1.1,
      theme: themeOf(cfg),
      cursorBlink: false,
      // tmux owns scrollback for tmux sessions.
      scrollback: kind === "tmux" ? 0 : 10000,
      macOptionClickForcesSelection: true,
      // Shift+drag does a native selection while tmux has the mouse.
      altClickMovesCursor: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const el = document.createElement("div");
    el.className = "term" + (kind === "tmux" ? " no-scrollbar" : "");
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
      ack: 0,
    };
    term.onData((d) => this.send(e, d));
    term.onResize(({ cols, rows }) => {
      if (e.pty !== null) api.resize(e.pty, cols, rows).catch(() => {});
    });
    term.attachCustomKeyEventHandler((ev) => this.keyHandler(e, ev));
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
      if (ev.type === "keydown") {
        readText()
          .then((t) => t && e.term.paste(t))
          .catch(() => {});
      }
      ev.preventDefault();
      return false;
    }
    return true;
  }

  private send(e: Entry, data: string) {
    if (e.pty !== null) api.write(e.pty, data).catch(() => {});
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
    try {
      e.connectedAt = Date.now();
      if (e.kind === "login") {
        e.pty = await api.openLogin(e.host, e.term.cols, e.term.rows, chan);
      } else {
        e.pty = await api.attach(e.host, e.slug, e.term.cols, e.term.rows, chan);
      }
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
      e = this.create(s.key, "tmux", s.host, s.slug);
      this.show(s.key);
      this.connect(e);
    } else {
      this.show(s.key);
    }
    this.evict();
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

  async openEphemeral(host: string, dir: string, name: string, kind: string) {
    const tmp = `pending:${++this.seq}`;
    const e = this.create(tmp, "ephemeral", host, "");
    this.show(tmp);
    this.fitEntry(e);
    this.setStatus(tmp, "connecting");
    const chan = this.channel(e);
    try {
      const r = await api.startEphemeral(host, dir, name, kind, e.term.cols, e.term.rows, chan);
      e.pty = r.pty;
      e.pendingInput = r.input || undefined;
      this.entries.delete(tmp);
      delete app.termStatus[tmp];
      e.key = r.key;
      e.slug = r.key.slice(host.length + 1);
      this.entries.set(r.key, e);
      this.setStatus(r.key, "live");
      if (app.active === tmp) this.show(r.key);
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

  applyConfig(cfg: Config) {
    for (const e of this.entries.values()) {
      e.term.options.fontFamily = cfg.font_family;
      e.term.options.fontSize = cfg.font_size;
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
