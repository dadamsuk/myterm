<script lang="ts">
  import { api, type HookPlan } from "../lib/api";
  import { app, toast } from "../lib/store.svelte";
  import { diffLines, hunks } from "../lib/diff";

  let { host, install }: { host: string; install: boolean } = $props();

  let plan = $state<HookPlan | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  $effect(() => {
    api
      .hooksPlan(host, install)
      .then((p) => (plan = p))
      .catch((e) => (error = String(e)));
  });

  let changes = $derived(
    plan?.files.map((f) => ({ path: f.path, lines: hunks(diffLines(f.before, f.after)), same: f.before === f.after })) ?? [],
  );
  let nothing = $derived(changes.length > 0 && changes.every((c) => c.same));

  async function apply() {
    busy = true;
    try {
      await api.hooksApply(host, install);
      toast(install ? `Attention hooks installed on ${host}` : `Attention hooks removed from ${host}`);
      app.overlay = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter" && plan && !busy) {
      e.preventDefault();
      apply();
    }
  }
</script>

<svelte:window onkeydown={key} />

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog wide" role="dialog" aria-label="Attention hooks">
    <div class="title">{install ? "Install" : "Uninstall"} attention hooks on {host}</div>
    <div class="body">
      {#if error}
        <div class="err">{error}</div>
      {:else if !plan}
        <div class="dim">Reading current settings…</div>
      {:else}
        {#if nothing}
          <div class="dim">Already up to date; nothing would change.</div>
        {/if}
        {#each changes as c}
          <div class="file">
            <div class="path">{c.path}{#if c.same}<span class="faint"> (unchanged)</span>{/if}</div>
            {#if !c.same}
              <pre>{#each c.lines as l}{#if l === null}<span class="gap">⋯</span>
{:else}<span class={l.op === "+" ? "add" : l.op === "-" ? "del" : ""}>{l.op} {l.text}</span>
{/if}{/each}</pre>
            {/if}
          </div>
        {/each}
        <div class="dim small">A backup is written next to settings.json as settings.json.myterm-bak.</div>
      {/if}
    </div>
    <div class="actions">
      <button class="btn" onclick={() => (app.overlay = null)}>Cancel</button>
      <button class="btn primary" disabled={!plan || busy || !!error} onclick={apply}>
        {busy ? "Applying…" : install ? "Install" : "Uninstall"}
      </button>
    </div>
  </div>
</div>

<style>
  .wide {
    width: min(860px, 94vw);
    max-height: 80vh;
  }
  .title {
    padding: 12px 16px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .body {
    padding: 12px 16px;
    overflow-y: auto;
    flex: 1;
    user-select: text;
    -webkit-user-select: text;
  }
  .file {
    margin-bottom: 14px;
  }
  .path {
    font-family: ui-monospace, monospace;
    font-size: 12px;
    margin-bottom: 4px;
  }
  pre {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 11.5px;
    line-height: 1.45;
    overflow-x: auto;
    max-height: 40vh;
  }
  .add {
    color: var(--green);
  }
  .del {
    color: var(--red);
  }
  .gap {
    color: var(--fg-faint);
  }
  .err {
    color: var(--red);
    white-space: pre-wrap;
  }
  .small {
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
