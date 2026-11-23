<script lang="ts">
  import type { HostInfo, ListMode, Session } from "../lib/api";
  import {
    app,
    basename,
    dotClass,
    groupedSessions,
    isReachable,
    lastUsed,
    relTime,
    setListMode,
    setSidebar,
  } from "../lib/store.svelte";
  import { activate, hostAction, setHostHidden } from "../lib/actions";
  import { terminals } from "../lib/terminals";

  let groups = $derived(groupedSessions());
  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30000);
    return () => clearInterval(t);
  });

  // `now` is passed so the label re-renders as time passes.
  const ago = (s: Session, _now: number) => relTime(lastUsed(s));

  const modes: { id: ListMode; label: string }[] = [
    { id: "machine", label: "Machine" },
    { id: "name", label: "Name" },
    { id: "recent", label: "Recent" },
  ];

  function hostDot(h: HostInfo): string {
    if (h.connecting) return "var(--fg-faint)";
    if (h.kind === "tmux" && h.connected) return "var(--green)";
    if (h.kind === "ephemeral") return "var(--accent)";
    if (h.needs_login) return "var(--yellow)";
    return "var(--red)";
  }

  function onMenu(e: MouseEvent, s: Session) {
    e.preventDefault();
    app.menu = { x: e.clientX, y: e.clientY, session: s };
  }

  // Resizing
  let dragging = false;
  function startResize(e: PointerEvent) {
    dragging = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }
  function moveResize(e: PointerEvent) {
    if (!dragging) return;
    app.sidebarWidth = Math.min(600, Math.max(180, e.clientX));
  }
  function endResize() {
    if (!dragging) return;
    dragging = false;
    setSidebar(false, app.sidebarWidth);
    terminals.fitActive();
  }

  let problemHosts = $derived(app.hosts.filter((h) => !h.hidden));
  let hostsOpen = $state(false);
</script>

<aside style="width: {app.sidebarWidth}px">
  <div class="modes" role="tablist">
    {#each modes as m}
      <button role="tab" class:on={app.listMode === m.id} aria-selected={app.listMode === m.id} onclick={() => setListMode(m.id)}>
        {m.label}
      </button>
    {/each}
  </div>

  <div class="list">
    {#each groups as g (g.title + (g.host?.alias ?? ""))}
      {#if g.title}
        <div class="group" class:attention={g.title === "Needs you" && !g.host}>
          {#if g.host}
            <span class="hdot" style="background: {hostDot(g.host)}"></span>
          {/if}
          <span class="gtitle">{g.title}</span>
          {#if g.host && !(g.host.kind === "tmux" && g.host.connected)}
            <button class="hstatus" title={g.host.status} onclick={() => g.host && hostAction(g.host)}>
              {g.host.needs_login ? "log in" : g.host.kind === "ephemeral" ? "ephemeral" : g.host.status}
            </button>
          {/if}
        </div>
      {/if}
      {#each g.sessions as s (s.key)}
        <button
          class="row"
          class:active={app.active === s.key}
          class:greyed={!isReachable(s)}
          class:eph={!s.persistent}
          onclick={() => activate(s)}
          oncontextmenu={(e) => onMenu(e, s)}
          title={`${s.display_name} — ${s.host}:${s.cwd}`}
        >
          <span class="dot {dotClass(s)}"></span>
          <span class="name">{s.display_name}</span>
          {#if !s.persistent}<span class="tag" title="Ends when the app quits">eph</span>{/if}
          {#if app.listMode !== "machine"}<span class="host">{s.host}</span>{/if}
          <span class="folder">{basename(s.cwd)}</span>
          <span class="time">{ago(s, now)}</span>
        </button>
      {/each}
      {#if g.host && g.sessions.length === 0}
        <div class="none-row faint">no sessions</div>
      {/if}
    {/each}
    {#if groups.every((g) => g.sessions.length === 0) && app.listMode !== "machine"}
      <div class="empty dim">
        No sessions yet.<br />Press <kbd>{app.config?.keys.new_session ?? "Ctrl+Shift+N"}</kbd> to start one.
      </div>
    {/if}
  </div>

  <div class="hosts">
    <button class="hosts-toggle" onclick={() => (hostsOpen = !hostsOpen)}>
      <span>Hosts</span>
      <span class="hsummary">
        {#each problemHosts as h (h.alias)}
          <span class="hdot" style="background: {hostDot(h)}" title="{h.alias}: {h.status}"></span>
        {/each}
      </span>
    </button>
    {#if hostsOpen}
      <div class="hostlist">
        {#each problemHosts as h (h.alias)}
          <div class="hrow">
            <span class="hdot" style="background: {hostDot(h)}"></span>
            <button class="hname" title="Click to {h.needs_login ? 'log in' : 'reconnect'}" onclick={() => hostAction(h)}>{h.alias}</button>
            <span class="hst" title={h.status}>{h.needs_login ? "needs interactive login" : h.status}</span>
            {#if !h.local}
              <button class="hide" title="Hide host" onclick={() => setHostHidden(h.alias, true)}>×</button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <div
    class="resizer"
    role="separator"
    aria-orientation="vertical"
    onpointerdown={startResize}
    onpointermove={moveResize}
    onpointerup={endResize}
  ></div>
</aside>

<style>
  aside {
    position: relative;
    flex-shrink: 0;
    background: var(--panel);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .modes {
    display: flex;
    margin: 10px 10px 6px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 2px;
  }
  .modes button {
    flex: 1;
    background: transparent;
    border: 0;
    padding: 4px 0;
    border-radius: 5px;
    cursor: pointer;
    color: var(--fg-dim);
    font-size: 12px;
  }
  .modes button.on {
    background: var(--selected);
    color: var(--fg);
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 0 6px 8px;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 6px 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-faint);
  }
  .group.attention .gtitle {
    color: var(--orange);
  }
  .gtitle {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hstatus {
    margin-left: auto;
    background: none;
    border: 0;
    color: var(--fg-faint);
    text-transform: none;
    letter-spacing: 0;
    font-size: 11px;
    cursor: pointer;
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hstatus:hover {
    color: var(--fg);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    background: transparent;
    border: 0;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
    min-width: 0;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.active {
    background: var(--selected);
  }
  .row.greyed {
    opacity: 0.45;
  }
  .row.eph .name {
    font-style: italic;
  }
  .name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .host {
    font-size: 11px;
    color: var(--accent);
    opacity: 0.8;
    flex-shrink: 0;
    max-width: 80px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .folder {
    font-size: 11px;
    color: var(--fg-faint);
    flex-shrink: 1;
    max-width: 90px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    font-size: 11px;
    color: var(--fg-faint);
    width: 26px;
    text-align: right;
    flex-shrink: 0;
  }
  .none-row {
    padding: 2px 14px 4px;
    font-size: 12px;
  }
  .empty {
    padding: 24px 12px;
    text-align: center;
    line-height: 1.8;
  }
  .hosts {
    border-top: 1px solid var(--border);
    font-size: 12px;
  }
  .hosts-toggle {
    display: flex;
    align-items: center;
    width: 100%;
    background: none;
    border: 0;
    padding: 8px 12px;
    cursor: pointer;
    color: var(--fg-dim);
    gap: 8px;
  }
  .hsummary {
    display: flex;
    gap: 3px;
    flex-wrap: wrap;
    margin-left: auto;
  }
  .hdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    display: inline-block;
  }
  .hostlist {
    max-height: 40vh;
    overflow-y: auto;
    padding: 0 6px 6px;
  }
  .hrow {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 3px 6px;
  }
  .hname {
    background: none;
    border: 0;
    padding: 0;
    cursor: pointer;
    flex-shrink: 0;
  }
  .hname:hover {
    text-decoration: underline;
  }
  .hst {
    color: var(--fg-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .hide {
    background: none;
    border: 0;
    color: var(--fg-faint);
    cursor: pointer;
    font-size: 14px;
    padding: 0 4px;
  }
  .hide:hover {
    color: var(--fg);
  }
  .resizer {
    position: absolute;
    top: 0;
    right: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 5;
  }
</style>
