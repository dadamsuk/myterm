<script lang="ts">
  import { app, isMac } from "../lib/store.svelte";
  import { terminals } from "../lib/terminals";
  import { label } from "../lib/keys";

  let menu = $derived(app.termMenu!);
  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ x: 0, y: 0 });
  const keys = app.config?.keys;
  const mac = isMac();

  $effect(() => {
    const m = menu;
    const w = el?.offsetWidth ?? 180;
    const h = el?.offsetHeight ?? 120;
    pos = { x: Math.min(m.x, window.innerWidth - w - 4), y: Math.min(m.y, window.innerHeight - h - 4) };
  });

  function run(fn: (key: string) => void) {
    const key = menu.key;
    app.termMenu = null;
    fn(key);
  }
</script>

<div class="catch" role="presentation" onpointerdown={() => (app.termMenu = null)}></div>
<div class="menu" bind:this={el} style="left: {pos.x}px; top: {pos.y}px" role="menu">
  <button role="menuitem" disabled={!menu.selection} onclick={() => run((k) => terminals.copy(k))}>
    <span>Copy</span>{#if keys}<kbd>{label(keys.copy, mac)}</kbd>{/if}
  </button>
  <button role="menuitem" onclick={() => run((k) => terminals.paste(k))}>
    <span>Paste</span>{#if keys}<kbd>{label(keys.paste, mac)}</kbd>{/if}
  </button>
  <button role="menuitem" onclick={() => run((k) => terminals.selectAll(k))}><span>Select all</span></button>
  {#if !terminals.isTmux(menu.key)}
    <button role="menuitem" onclick={() => run((k) => terminals.clear(k))}><span>Clear scrollback</span></button>
  {:else if !menu.selection}
    <div class="note">Shift+drag selects text in tmux</div>
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
    min-width: 180px;
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 4px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
  }
  button {
    display: flex;
    align-items: center;
    gap: 16px;
    background: none;
    border: 0;
    text-align: left;
    padding: 6px 10px;
    border-radius: 5px;
    cursor: pointer;
  }
  button span {
    flex: 1;
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  kbd {
    font-size: 11px;
  }
  .note {
    padding: 6px 10px 4px;
    font-size: 11px;
    color: var(--fg-faint);
    border-top: 1px solid var(--border);
    margin-top: 4px;
  }
</style>
