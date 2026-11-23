<script lang="ts">
  import { app } from "../lib/store.svelte";
  import { copyAttachCommand, killSession } from "../lib/actions";

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
  <button role="menuitem" onclick={() => { const s = menu.session; close(); app.overlay = { kind: "rename", session: s }; }}>Rename…</button>
  <button role="menuitem" class="danger" onclick={() => killSession(menu.session)}>Kill</button>
  {#if menu.session.persistent}
    <button role="menuitem" onclick={() => copyAttachCommand(menu.session)}>Copy attach command</button>
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
