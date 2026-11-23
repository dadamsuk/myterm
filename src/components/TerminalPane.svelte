<script lang="ts">
  import { onMount } from "svelte";
  import { app, sessionByKey } from "../lib/store.svelte";
  import { terminals } from "../lib/terminals";
  import { dismissEnded } from "../lib/actions";
  import { label } from "../lib/keys";

  let container: HTMLDivElement;
  onMount(() => terminals.mount(container));

  let status = $derived(app.active ? app.termStatus[app.active] : undefined);
  let mac = $derived(app.platform === "macos");
</script>

<div class="pane">
  <div class="terms" bind:this={container}></div>

  {#if !app.active && app.ready}
    <div class="welcome">
      <h1>myterm</h1>
      <div class="keys">
        <div><kbd>{label(app.config?.keys.switcher ?? "", mac)}</kbd><span>Switch session</span></div>
        <div><kbd>{label(app.config?.keys.new_session ?? "", mac)}</kbd><span>New Claude session</span></div>
        <div><kbd>{label(app.config?.keys.palette ?? "", mac)}</kbd><span>Command palette</span></div>
      </div>
    </div>
  {/if}

  {#if app.active && status && status.status !== "live"}
    {#if status.status === "connecting"}
      <div class="banner">Connecting…</div>
    {:else if status.status === "reconnecting"}
      <div class="veil">
        <div class="card">
          <div class="spinner"></div>
          <div>Reconnecting</div>
          <div class="dim small">{status.message}</div>
          <button class="btn" onclick={() => app.active && terminals.reattach(app.active)}>Retry now</button>
        </div>
      </div>
    {:else if status.status === "detached"}
      <div class="veil">
        <div class="card">
          <div>Detached</div>
          <div class="btns">
            <button class="btn primary" onclick={() => app.active && terminals.reattach(app.active)}>Reattach</button>
            <button class="btn" onclick={() => app.active && terminals.dispose(app.active)}>Close</button>
          </div>
        </div>
      </div>
    {:else}
      <div class="endbar">
        <span>{status.message ?? "Ended"}</span>
        {#if app.active && sessionByKey(app.active)?.persistent}
          <button class="btn" onclick={() => app.active && terminals.reattach(app.active)}>Reattach</button>
        {/if}
        <button class="btn" onclick={() => app.active && dismissEnded(app.active)}>Dismiss</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .pane {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .terms {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }
  .welcome {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 18px;
    color: var(--fg-dim);
  }
  .welcome h1 {
    font-weight: 500;
    font-size: 22px;
    color: var(--fg-faint);
    margin: 0;
  }
  .keys {
    display: grid;
    gap: 8px;
  }
  .keys div {
    display: grid;
    grid-template-columns: 120px auto;
    gap: 12px;
    align-items: center;
  }
  .keys kbd {
    justify-self: end;
  }
  .banner {
    position: absolute;
    top: 10px;
    right: 14px;
    font-size: 12px;
    color: var(--fg-dim);
    background: var(--panel-2);
    border: 1px solid var(--border);
    padding: 3px 10px;
    border-radius: 5px;
  }
  .veil {
    position: absolute;
    inset: 0;
    background: rgba(10, 11, 14, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 18px 26px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    max-width: 70%;
    text-align: center;
  }
  .small {
    font-size: 12px;
  }
  .btns {
    display: flex;
    gap: 8px;
  }
  .spinner {
    width: 18px;
    height: 18px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .endbar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 14px;
    background: var(--panel);
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  .endbar span {
    flex: 1;
  }
</style>
