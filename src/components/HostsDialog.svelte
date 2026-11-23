<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, type HostEntry } from "../lib/api";
  import { app, hostInfo } from "../lib/store.svelte";
  import { hostAction, hostEntries, openSshConfig, saveHosts } from "../lib/actions";
  import { hostColor, hostStatus } from "../lib/hostdot";

  // Edited here and written through on every change; the snapshot that
  // follows carries the new order back into app.hosts.
  let list = $state<HostEntry[]>(untrack(hostEntries));
  let aliases = $state<string[]>([]);
  let name = $state("");
  let autoConnect = $state(true);
  let input: HTMLInputElement;

  onMount(() => {
    input.focus();
    api.sshAliases().then((a) => (aliases = a)).catch(() => {});
  });

  let suggestions = $derived(
    ["local", ...aliases].filter((a) => !list.some((h) => h.name === a)),
  );

  function commit(next: HostEntry[]) {
    list = next;
    saveHosts($state.snapshot(next));
  }

  function add() {
    const n = name.trim();
    if (!n) return;
    if (!list.some((h) => h.name === n)) commit([...list, { name: n, auto_connect: autoConnect }]);
    name = "";
    input.focus();
  }

  function move(i: number, d: number) {
    const j = i + d;
    if (j < 0 || j >= list.length) return;
    const next = [...list];
    [next[i], next[j]] = [next[j], next[i]];
    commit(next);
  }

  function setAuto(i: number, on: boolean) {
    commit(list.map((h, k) => (k === i ? { ...h, auto_connect: on } : h)));
  }

  function remove(i: number) {
    commit(list.filter((_, k) => k !== i));
  }

  // Drag to reorder by the handle. Pointer events rather than HTML drag and
  // drop, which the webview's file-drop handling takes over.
  let dragFrom = $state<number | null>(null);
  let rowsEl: HTMLDivElement;
  function dragStart(e: PointerEvent, i: number) {
    dragFrom = i;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function dragMove(e: PointerEvent) {
    if (dragFrom === null) return;
    const rows = [...rowsEl.querySelectorAll<HTMLElement>(".hrow")];
    const to = rows.findIndex((r) => {
      const b = r.getBoundingClientRect();
      return e.clientY >= b.top && e.clientY < b.bottom;
    });
    if (to < 0 || to === dragFrom) return;
    const next = [...list];
    const [x] = next.splice(dragFrom, 1);
    next.splice(to, 0, x);
    list = next;
    dragFrom = to;
  }
  function dragEnd() {
    if (dragFrom === null) return;
    dragFrom = null;
    commit(list);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog" aria-label="Hosts">
    <div class="head">
      <span class="title">Hosts</span>
      <span class="faint">shown in this order</span>
    </div>

    <div class="rows" bind:this={rowsEl}>
      {#each list as h, i (h.name)}
        {@const info = hostInfo(h.name)}
        <div class="hrow" class:dragging={dragFrom === i}>
          <span
            class="grip"
            title="Drag to reorder"
            role="button"
            tabindex="-1"
            onpointerdown={(e) => dragStart(e, i)}
            onpointermove={dragMove}
            onpointerup={dragEnd}
            onpointercancel={dragEnd}
          >⋮⋮</span>
          <span class="hdot {info ? hostColor(info) : 'connecting'}"></span>
          <span class="name">{h.name}</span>
          {#if info}
            <button class="st" title={info.status} onclick={() => hostAction(info)}>{hostStatus(info)}</button>
          {:else}
            <span class="st faint">adding…</span>
          {/if}
          <label class="auto" title="Connect when myterm starts. Off: connect on first click.">
            <input type="checkbox" checked={h.auto_connect} onchange={(e) => setAuto(i, e.currentTarget.checked)} />
            auto-connect
          </label>
          <button class="icon" title="Move up" aria-label="Move up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
          <button class="icon" title="Move down" aria-label="Move down" disabled={i === list.length - 1} onclick={() => move(i, 1)}>↓</button>
          <button class="icon x" title="Remove from the list" aria-label="Remove" onclick={() => remove(i)}>×</button>
        </div>
      {:else}
        <div class="empty">No hosts. Add one below.</div>
      {/each}
    </div>

    <div class="add">
      <input
        bind:this={input}
        bind:value={name}
        onkeydown={key}
        list="ssh-aliases"
        placeholder="Host: an ssh config alias such as intra, or user@server"
        spellcheck="false"
        autocomplete="off"
      />
      <datalist id="ssh-aliases">
        {#each suggestions as a (a)}<option value={a}></option>{/each}
      </datalist>
      <label class="auto" title="Connect when myterm starts">
        <input type="checkbox" bind:checked={autoConnect} />
        auto-connect
      </label>
      <button class="btn primary" disabled={!name.trim()} onclick={add}>Add</button>
    </div>
    {#if suggestions.length}
      <div class="chips">
        <span class="faint">From ~/.ssh/config:</span>
        {#each suggestions as a (a)}
          <button class="chip" onclick={() => ((name = a), add())}>{a}</button>
        {/each}
      </div>
    {/if}
    <div class="foot faint">
      Saved to config.toml as <code>[[host]]</code> entries, which you can also edit by hand.
      Connection details (HostName, User, keys) stay in
      <button class="link" onclick={openSshConfig}>~/.ssh/config</button>.
    </div>
  </div>
</div>

<style>
  .dialog {
    width: min(680px, 94vw);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 12px 14px 8px;
  }
  .title {
    font-size: 14px;
    font-weight: 600;
  }
  .rows {
    overflow-y: auto;
    padding: 0 6px 6px;
    min-height: 40px;
  }
  .hrow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-radius: 6px;
  }
  .hrow:hover {
    background: var(--hover);
  }
  .hrow.dragging {
    background: var(--selected);
  }
  .grip {
    cursor: grab;
    color: var(--fg-faint);
    font-size: 11px;
    letter-spacing: -2px;
    padding: 0 2px;
    touch-action: none;
  }
  .hrow.dragging .grip {
    cursor: grabbing;
  }
  .name {
    flex-shrink: 0;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    padding: 0;
    text-align: left;
    color: var(--fg-faint);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .st:hover {
    color: var(--fg);
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--fg-dim);
    flex-shrink: 0;
    cursor: pointer;
  }
  .icon {
    background: none;
    border: 0;
    color: var(--fg-dim);
    cursor: pointer;
    padding: 2px 5px;
    border-radius: 4px;
    font-size: 13px;
  }
  .icon:hover:not(:disabled) {
    background: var(--panel-2);
    color: var(--fg);
  }
  .icon:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .icon.x {
    font-size: 15px;
  }
  .add {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
  }
  .add input:not([type]) {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 9px;
    outline: none;
  }
  .add input:not([type]):focus {
    border-color: var(--accent);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    align-items: center;
    padding: 0 14px 10px;
    font-size: 12px;
  }
  .chip {
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1px 9px;
    font-size: 12px;
    cursor: pointer;
  }
  .chip:hover {
    background: var(--hover);
  }
  .foot {
    padding: 8px 14px 12px;
    font-size: 12px;
    line-height: 1.5;
    border-top: 1px solid var(--border);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: inherit;
  }
  .link:hover {
    text-decoration: underline;
  }
  code {
    font-size: 11.5px;
  }
</style>
