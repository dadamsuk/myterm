<script lang="ts">
  import { activeRow, app, hostInfo, multiSession, rowCwd, rowName, rowState, rowsOf, sessionByKey } from "../lib/store.svelte";
  import { toggleSidebar } from "../lib/actions";

  let session = $derived(sessionByKey(app.active));
  // The window on screen: its name, folder and state.
  let row = $derived(activeRow() ?? (session ? rowsOf(session)[0] : undefined));
  let loginHost = $derived(app.active?.startsWith("login:") ? app.active.slice(6) : null);
  let host = $derived(hostInfo(session?.host ?? loginHost ?? ""));
  let term = $derived(app.active ? app.termStatus[app.active] : undefined);

  const stateLabel: Record<string, string> = {
    working: "working",
    permission: "waiting for permission",
    done: "done",
  };

  let conn = $derived.by(() => {
    if (!term) return "";
    if (term.status === "live") return host?.kind === "tmux" && !host.connected ? "host channel down" : "connected";
    return term.status;
  });
</script>

<header>
  <button class="toggle" title="Toggle sidebar" aria-label="Toggle sidebar" onclick={toggleSidebar}>
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
      <line x1="6" y1="2.5" x2="6" y2="13.5" />
    </svg>
  </button>
  {#if session && row}
    {@const st = rowState(row)}
    <span class="title">{rowName(row)}</span>
    <span class="tag">{session.host}</span>
    {#if session.persistent && multiSession(session.host)}<span class="win">{session.display_name}</span>{/if}
    {#if !session.persistent}<span class="tag eph">ephemeral</span>{/if}
    <span class="cwd" title={rowCwd(row)}>{rowCwd(row)}</span>
    {#if st && !session.ended}
      <span class="state {st}"><span class="dot {st}"></span>{stateLabel[st] ?? st}</span>
    {/if}
    <span class="conn" class:bad={conn !== "connected"}>{conn}</span>
  {:else if loginHost}
    <span class="title">Log in to {loginHost}</span>
    <span class="conn">{conn}</span>
  {:else}
    <span class="dim">myterm</span>
  {/if}
</header>

<style>
  header {
    height: 34px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 0 12px 0 6px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    overflow: hidden;
    -webkit-app-region: drag;
  }
  .toggle {
    background: none;
    border: 0;
    color: var(--fg-dim);
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 5px;
    display: flex;
  }
  .toggle:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .title {
    font-weight: 600;
  }
  .win {
    color: var(--fg-faint);
    font-size: 12px;
  }
  .tag.eph {
    color: var(--yellow);
  }
  .cwd {
    color: var(--fg-faint);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-dim);
  }
  .state.permission {
    color: var(--orange);
  }
  .conn {
    margin-left: auto;
    font-size: 11.5px;
    color: var(--fg-faint);
  }
  .conn.bad {
    color: var(--yellow);
  }
</style>
