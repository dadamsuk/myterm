<script lang="ts">
  // config.toml edited in the app (spec 14.4): Android has no editor to open
  // the file in. Save checks the text parses before writing it.
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { app, toast } from "../lib/store.svelte";

  let text = $state("");
  let saved = $state("");
  let error = $state<string | null>(null);
  let loaded = $state(false);
  let busy = $state(false);

  onMount(async () => {
    try {
      text = saved = await api.readConfigText();
      loaded = true;
    } catch (e) {
      error = String(e);
    }
  });

  async function save() {
    busy = true;
    try {
      await api.writeConfigText(text);
      saved = text;
      error = null;
      toast("Settings saved");
      app.overlay = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" class:phone={app.phone} role="dialog" aria-label="Settings">
    <div class="head">
      <span class="title">Settings</span>
      <span class="faint">config.toml</span>
      <span class="grow"></span>
      <button class="btn" onclick={() => (app.overlay = null)}>{text === saved ? "Close" : "Cancel"}</button>
      <button class="btn primary" disabled={!loaded || busy || text === saved} onclick={save}>Save</button>
    </div>
    {#if error}<div class="error">{error}</div>{/if}
    <textarea
      bind:value={text}
      disabled={!loaded}
      spellcheck="false"
      autocomplete="off"
      autocapitalize="none"
      wrap={app.phone ? "soft" : "off"}
    ></textarea>
  </div>
</div>

<style>
  .dialog {
    width: min(760px, 94vw);
    height: 76vh;
  }
  .dialog.phone {
    position: fixed;
    inset: 0;
    width: auto;
    height: auto;
    max-height: none;
    border-radius: 0;
    border: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    font-size: 14px;
    font-weight: 600;
  }
  .grow {
    flex: 1;
  }
  .btn:disabled {
    opacity: 0.5;
  }
  .error {
    padding: 8px 12px;
    color: #f7768e;
    font-size: 12px;
    white-space: pre-wrap;
    border-bottom: 1px solid var(--border);
  }
  textarea {
    flex: 1;
    min-height: 0;
    resize: none;
    border: 0;
    outline: none;
    padding: 10px 12px;
    background: var(--bg);
    color: var(--fg);
    font-family: ui-monospace, "Cascadia Mono", Menlo, monospace;
    font-size: 13px;
    line-height: 1.45;
    tab-size: 2;
  }
</style>
