<script lang="ts">
  import type { ListMode } from "../lib/api";
  import {
    app,
    basename,
    groupedRows,
    isMac,
    isReachable,
    multiSession,
    relTime,
    rowCwd,
    rowDot,
    rowLastUsed,
    rowName,
    rowNeedsAttention,
    rowOnScreen,
    rowTooltip,
    setListMode,
    setSidebar,
    type Row,
  } from "../lib/store.svelte";
  import { activateRow, hostAction, manageHosts, openSettings, removeHost, toggleAutoHide, toggleSidebar } from "../lib/actions";
  import { hostColor, hostStatus } from "../lib/hostdot";

  let { floating = false }: { floating?: boolean } = $props();
  import { label } from "../lib/keys";
  import { terminals } from "../lib/terminals";

  let groups = $derived(groupedRows());
  // 1-based position in the list; the first nine switch with Ctrl+Shift+N.
  let numbers = $derived(new Map(groups.flatMap((g) => g.rows).map((r, i) => [r.key, i + 1])));
  const keys = $derived(app.config?.keys);
  const hint = (b: string | undefined) => (b ? ` (${label(b, isMac())})` : "");
  let now = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30000);
    return () => clearInterval(t);
  });

  // `now` is passed so the label re-renders as time passes.
  const ago = (r: Row, _now: number) => relTime(rowLastUsed(r));

  const modes: { id: ListMode; label: string }[] = [
    { id: "machine", label: "Machine" },
    { id: "name", label: "Name" },
    { id: "recent", label: "Recent" },
  ];

  function onMenu(e: MouseEvent, r: Row) {
    e.preventDefault();
    app.menu = { x: e.clientX, y: e.clientY, row: r };
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

  let hostsOpen = $state(false);
</script>

<aside class:floating style="width: {app.sidebarWidth}px">
  <div class="tools">
    <button title="New session{hint(keys?.new_session)}" aria-label="New session" onclick={() => (app.overlay = { kind: "new" })}>
      <svg viewBox="0 0 16 16"><path d="M8 3v10M3 8h10" /></svg>
    </button>
    <button title="Hosts: add, order, auto-connect" aria-label="Hosts" onclick={manageHosts}>
      <svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="10" height="4" rx="1" /><rect x="1.5" y="9.5" width="10" height="4" rx="1" /><path d="M13.5 9v5M11 11.5h5" /></svg>
    </button>
    <button title="Switch session{hint(keys?.switcher)}" aria-label="Switch session" onclick={() => (app.overlay = { kind: "switcher" })}>
      <svg viewBox="0 0 16 16"><path d="M2 5h11M10 2l3 3-3 3M14 11H3M6 8l-3 3 3 3" /></svg>
    </button>
    <button title="Command palette{hint(keys?.palette)}" aria-label="Command palette" onclick={() => (app.overlay = { kind: "palette" })}>
      <svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
    </button>
    <button title="Settings (opens config.toml)" aria-label="Settings" onclick={openSettings}>
      <svg viewBox="0 0 16 16"><path d="M6.9 1.5h2.2l.4 1.8 1.2.5 1.6-1 1.5 1.5-1 1.6.5 1.2 1.8.4v2.2l-1.8.4-.5 1.2 1 1.6-1.5 1.5-1.6-1-1.2.5-.4 1.8H6.9l-.4-1.8-1.2-.5-1.6 1-1.5-1.5 1-1.6-.5-1.2-1.8-.4V6.9l1.8-.4.5-1.2-1-1.6 1.5-1.5 1.6 1 1.2-.5z" /><circle cx="8" cy="8" r="2.1" /></svg>
    </button>
    <button
      class="collapse"
      class:on={app.sidebarAutoHide}
      title={app.sidebarAutoHide
        ? "Keep the sidebar open"
        : `Auto-hide the sidebar: it comes back when you point at the left edge${hint(keys?.peek_sidebar)}`}
      aria-label="Auto-hide sidebar"
      aria-pressed={app.sidebarAutoHide}
      onclick={toggleAutoHide}
    >
      <svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="2" /><path d="M6 2.5v11" stroke-dasharray={app.sidebarAutoHide ? "none" : "1.6 1.6"} /><path d="M3 6h1.2M3 8h1.2M3 10h1.2" /></svg>
    </button>
    {#if !app.sidebarAutoHide}
      <button title="Hide sidebar{hint(keys?.toggle_sidebar)}" aria-label="Hide sidebar" onclick={toggleSidebar}>
        <svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="2" /><path d="M6 2.5v11M11 6 9 8l2 2" /></svg>
      </button>
    {/if}
  </div>
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
            <span class="hdot {hostColor(g.host)}"></span>
          {/if}
          <span class="gtitle">{g.title}</span>
          {#if g.host && !(g.host.kind === "tmux" && g.host.connected)}
            <button class="hstatus" title={g.host.status} onclick={() => g.host && hostAction(g.host)}>
              {g.host.dormant ? "connect" : g.host.needs_login ? "log in" : g.host.kind === "ephemeral" ? "ephemeral" : g.host.status}
            </button>
          {/if}
          {#if g.host && ((g.host.kind === "tmux" && g.host.connected) || g.host.kind === "ephemeral")}
            {@const alias = g.host.alias}
            <button
              class="add"
              title="New session on {alias}"
              aria-label="New session on {alias}"
              onclick={() => (app.overlay = { kind: "new", host: alias })}
            >+</button>
          {/if}
        </div>
      {/if}
      {#each g.rows as r (r.key)}
        <button
          class="row"
          class:active={rowOnScreen(r)}
          class:greyed={!isReachable(r.s)}
          class:eph={!r.s.persistent}
          class:needs-permission={rowNeedsAttention(r) && rowDot(r) === "permission"}
          class:needs-done={rowNeedsAttention(r) && rowDot(r) === "done"}
          onclick={() => activateRow(r)}
          oncontextmenu={(e) => onMenu(e, r)}
          title={rowTooltip(r)}
        >
          <span class="num">{(numbers.get(r.key) ?? 99) <= 9 ? numbers.get(r.key) : ""}</span>
          <span class="dot {rowDot(r)}"></span>
          <span class="name" class:full={app.listMode === "name"}>{rowName(r)}</span>
          {#if app.listMode !== "name"}
          {#if !r.s.persistent}<span class="tag" title="Ends when the app quits">eph</span>{/if}
          {#if r.s.persistent && multiSession(r.s.host)}<span class="sess" title="tmux session">{r.s.display_name}</span>{/if}
          {#if app.listMode !== "machine"}<span class="host">{r.s.host}</span>{/if}
          <span class="folder">{basename(rowCwd(r))}</span>
          <span class="time">{ago(r, now)}</span>
          {/if}
        </button>
      {/each}
      {#if g.host && g.rows.length === 0}
        <div class="none-row faint">{g.host.dormant ? "not connected" : "no sessions"}</div>
      {/if}
    {/each}
    {#if groups.every((g) => g.rows.length === 0) && app.listMode !== "machine"}
      <div class="empty dim">
        No sessions yet.<br />Press <kbd>{app.config?.keys.new_session ?? "Ctrl+Shift+N"}</kbd> to start one.
      </div>
    {/if}
  </div>

  <div class="hosts">
    <button class="hosts-toggle" onclick={() => (hostsOpen = !hostsOpen)}>
      <span>Hosts</span>
      <span class="hsummary">
        {#each app.hosts as h (h.alias)}
          <span class="hdot {hostColor(h)}" title="{h.alias}: {hostStatus(h)}"></span>
        {/each}
      </span>
    </button>
    {#if hostsOpen}
      <div class="hostlist">
        {#each app.hosts as h (h.alias)}
          <div class="hrow">
            <span class="hdot {hostColor(h)}"></span>
            <button class="hname" title="Click to {h.dormant ? 'connect' : h.needs_login ? 'log in' : 'reconnect'}" onclick={() => hostAction(h)}>{h.alias}</button>
            <span class="hst" title={h.status}>{hostStatus(h)}</span>
            <button class="hide" title="Remove from the host list" onclick={() => removeHost(h.alias)}>×</button>
          </div>
        {/each}
        <button class="manage" onclick={manageHosts}>Add, order and auto-connect…</button>
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
    height: 100%;
    flex-shrink: 0;
    background: var(--panel);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  /* Auto-hide: over the terminal rather than beside it. */
  aside.floating {
    box-shadow: 8px 0 28px rgba(0, 0, 0, 0.45);
    animation: slide-in 0.12s ease-out;
  }
  @keyframes slide-in {
    from {
      transform: translateX(-24px);
      opacity: 0.4;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    aside.floating {
      animation: none;
    }
  }
  .tools {
    display: flex;
    gap: 2px;
    padding: 8px 8px 0;
  }
  .tools button {
    background: none;
    border: 0;
    color: var(--fg-dim);
    cursor: pointer;
    padding: 5px;
    border-radius: 5px;
    display: flex;
  }
  .tools button:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .tools .collapse {
    margin-left: auto;
  }
  .tools button.on {
    color: var(--accent);
  }
  .tools svg {
    width: 15px;
    height: 15px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .num {
    width: 9px;
    flex-shrink: 0;
    font-size: 11px;
    color: var(--fg-faint);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .name.full {
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .modes {
    display: flex;
    margin: 6px 10px 6px;
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
    color: var(--st-permission);
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
  .add {
    margin-left: auto;
    background: none;
    border: 0;
    color: var(--fg-faint);
    font-size: 15px;
    line-height: 1;
    padding: 0 4px;
    border-radius: 4px;
    cursor: pointer;
  }
  .hstatus + .add {
    margin-left: 4px;
  }
  .add:hover {
    color: var(--fg);
    background: var(--hover);
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
  /* Needs you: highlighted in place rather than moved. */
  .row.needs-permission {
    background: rgba(255, 138, 31, 0.14);
    box-shadow: inset 3px 0 0 var(--st-permission);
  }
  .row.needs-done {
    background: rgba(61, 220, 132, 0.1);
    box-shadow: inset 3px 0 0 var(--st-done);
  }
  .row.needs-permission:hover,
  .row.needs-done:hover {
    filter: brightness(1.15);
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
  .sess {
    font-size: 11px;
    color: var(--fg-faint);
    flex-shrink: 0;
    max-width: 70px;
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
  .manage {
    display: block;
    background: none;
    border: 0;
    color: var(--accent);
    font-size: 12px;
    padding: 4px 6px 2px;
    cursor: pointer;
  }
  .manage:hover {
    text-decoration: underline;
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
