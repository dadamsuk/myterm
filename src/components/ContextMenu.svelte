<script lang="ts">
  import { app } from "../lib/store.svelte";
  import { copyAttachCommand, findSaved, killRow } from "../lib/actions";
  import { terminals } from "../lib/terminals";
  import { rowName } from "../lib/store.svelte";

  let menu = $derived(app.menu!);
  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ x: 0, y: 0 });

  $effect(() => {
    const m = menu;
    const w = el?.offsetWidth ?? 200;
    const h = el?.offsetHeight ?? 100;
    pos = { x: Math.min(m.x, window.innerWidth - w - 4), y: Math.min(m.y, window.innerHeight - h - 4) };
  });

  function close() {
    app.menu = null;
  }
</script>

<div class="catch" role="presentation" onpointerdown={close}></div>
<div class="menu" bind:this={el} style="left: {pos.x}px; top: {pos.y}px" role="menu">
  <button role="menuitem" onclick={() => { const row = menu.row; close(); app.overlay = { kind: "rename", row }; }}>Rename…</button>
  <button role="menuitem" class="danger" onclick={() => killRow(menu.row)}>
    {menu.row.w && menu.row.s.windows.length > 1 ? "Kill window" : "Kill"}
  </button>
  <button role="menuitem" onclick={() => { const row = menu.row; close(); app.overlay = { kind: "save", row }; }}>
    {findSaved(menu.row.s.host, rowName(menu.row)) ? "Edit saved session…" : "Save session…"}
  </button>
  {#if menu.row.s.persistent}
    <button
      role="menuitem"
      title="Attach again with a new terminal, as reopening myterm does, when the screen is garbled"
      onclick={() => { const key = menu.row.s.key; close(); terminals.refresh(key); }}>Refresh</button
    >
    <button role="menuitem" onclick={() => copyAttachCommand(menu.row.s)}>Copy attach command</button>
  {/if}
</div>

<style>
  .catch {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 190px;
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 4px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
  }
  button {
    background: none;
    border: 0;
    text-align: left;
    padding: 6px 10px;
    border-radius: 5px;
    cursor: pointer;
  }
  button:hover {
    background: var(--hover);
  }
  .danger {
    color: var(--red);
  }
</style>
