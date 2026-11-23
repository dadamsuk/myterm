<script lang="ts">
  import { api } from "../lib/api";
  import { app } from "../lib/store.svelte";

  let { count }: { count: number } = $props();
  let cancel: HTMLButtonElement;
  $effect(() => cancel.focus());
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="alertdialog" aria-label="Quit">
    <div class="body">
      <b>Quit myterm?</b>
      <p class="dim">
        {count} ephemeral session{count === 1 ? " is" : "s are"} running. {count === 1 ? "It" : "They"} will end when the app quits.
      </p>
    </div>
    <div class="actions">
      <button class="btn" bind:this={cancel} onclick={() => (app.overlay = null)}>Cancel</button>
      <button class="btn primary" onclick={() => api.quit()}>Quit</button>
    </div>
  </div>
</div>

<style>
  .body {
    padding: 16px 18px 4px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 16px 14px;
  }
</style>
