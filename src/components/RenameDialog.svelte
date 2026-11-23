<script lang="ts">
  import { untrack } from "svelte";
  import type { Session } from "../lib/api";
  import { app } from "../lib/store.svelte";
  import { renameSession } from "../lib/actions";

  let { session }: { session: Session } = $props();
  let name = $state(untrack(() => session.display_name));
  let input: HTMLInputElement;

  $effect(() => {
    input.focus();
    input.select();
  });

  function key(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      app.overlay = null;
      renameSession(session, name);
    }
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog" aria-label="Rename session">
    <input class="query" bind:this={input} bind:value={name} onkeydown={key} spellcheck="false" />
    <div class="empty">Rename <b>{session.display_name}</b> on {session.host}. The tmux session name stays <code>{session.slug}</code>.</div>
  </div>
</div>
