<script lang="ts">
  import { untrack } from "svelte";
  import { app, rowName, type Row } from "../lib/store.svelte";
  import { renameRow } from "../lib/actions";

  let { row }: { row: Row } = $props();
  let name = $state(untrack(() => rowName(row)));
  let input: HTMLInputElement;

  $effect(() => {
    input.focus();
    input.select();
  });

  function key(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      // `row` reads app.overlay, so take it before closing the dialog.
      const r = row;
      app.overlay = null;
      renameRow(r, name);
    }
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog" aria-label="Rename">
    <input class="query" bind:this={input} bind:value={name} onkeydown={key} spellcheck="false" />
    <div class="empty">
      {#if row.w}
        Rename window <b>{row.w.name}</b> on {row.s.host}. This renames the tmux window, so tmux shows the new name too.
      {:else}
        Rename <b>{row.s.display_name}</b> on {row.s.host}.
      {/if}
    </div>
  </div>
</div>
