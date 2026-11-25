<script lang="ts">
  import { untrack } from "svelte";
  import type { SavedSession } from "../lib/api";
  import { app, rowCwd, rowName, type Row } from "../lib/store.svelte";
  import { findSaved, removeSaved, saveSession } from "../lib/actions";
  import { terminals } from "../lib/terminals";

  let { row, saved }: { row?: Row; saved?: SavedSession } = $props();

  // The dialog is created fresh each time, so reading the props once is intended.
  const r = untrack(() => row);
  const host = untrack(() => saved?.host) ?? r?.s.host ?? "local";
  const kind = untrack(() => saved?.kind) ?? (r?.w ? r.w.kind : r?.s.kind) ?? "shell";
  // Saving a running session again edits its saved entry rather than starting over.
  const existing = untrack(() => saved) ?? (r ? findSaved(host, rowName(r)) : undefined);
  const typed = r && !existing ? terminals.typedLines(r.s.key) : { lines: [], skipped: 0 };

  let name = $state(existing?.name ?? (r ? rowName(r) : ""));
  let cwd = $state(existing?.cwd ?? (r ? rowCwd(r) : ""));
  let commands = $state((existing?.commands ?? typed.lines).join("\n"));
  let autostart = $state(existing?.autostart ?? false);
  let input: HTMLInputElement;

  $effect(() => {
    input.focus();
    input.select();
  });

  function save() {
    const n = name.trim();
    if (!n) return input.focus();
    const sv: SavedSession = {
      name: n,
      host,
      kind,
      cwd: cwd.trim(),
      commands: commands
        .split(/\r?\n/)
        .map((c) => c.trim())
        .filter(Boolean),
      autostart,
    };
    app.overlay = null;
    saveSession(sv, existing);
  }

  function remove() {
    if (!existing) return;
    app.overlay = null;
    removeSaved(existing);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey || !(e.target instanceof HTMLTextAreaElement))) {
      e.preventDefault();
      save();
    }
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog" aria-label="Save session">
    <div class="title">
      {existing ? "Edit saved session" : "Save session"}
      <span class="faint">{kind === "claude" ? "Claude" : "shell"} on {host}</span>
    </div>
    <label>
      <span>Name</span>
      <input bind:this={input} bind:value={name} onkeydown={key} spellcheck="false" autocomplete="off" />
    </label>
    <label>
      <span>Folder</span>
      <input bind:value={cwd} onkeydown={key} placeholder="home folder" spellcheck="false" autocomplete="off" />
    </label>
    {#if kind === "shell"}
      <label class="top">
        <span>Commands</span>
        <textarea
          bind:value={commands}
          onkeydown={key}
          rows="5"
          spellcheck="false"
          placeholder={"One per line, typed in turn. For example:\nwsl\ncd ~/api && ./run.sh"}
        ></textarea>
      </label>
      <div class="note dim">
        Each line is typed once the previous one's output has gone quiet, so <code>wsl</code> and then a
        command for the Linux shell works.
        {#if !existing && r}
          {#if typed.lines.length}Filled in from what you typed in this session; check it before saving.{/if}
          {#if typed.skipped}
            <span class="warn">{typed.skipped} line{typed.skipped === 1 ? "" : "s"} used history or Tab completion and
              {typed.skipped === 1 ? "was" : "were"} left out: add {typed.skipped === 1 ? "it" : "them"} yourself.</span>
          {/if}
        {/if}
      </div>
    {/if}
    <label class="check">
      <input type="checkbox" bind:checked={autostart} />
      Start when myterm starts, unless a session called <b>{name.trim() || "…"}</b> is already running
    </label>
    <div class="actions">
      {#if existing}<button class="btn danger" onclick={remove}>Remove</button>{/if}
      <span class="faint grow">Start later from the palette: <b>Start saved sessions</b></span>
      <button class="btn" onclick={() => (app.overlay = null)}>Cancel</button>
      <button class="btn primary" disabled={!name.trim()} onclick={save}>Save</button>
    </div>
  </div>
</div>

<style>
  .title {
    padding: 12px 14px 8px;
    font-weight: 600;
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  .title .faint {
    font-weight: 400;
    font-size: 12px;
  }
  label {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 14px;
  }
  label.top {
    align-items: flex-start;
  }
  label.top span {
    padding-top: 7px;
  }
  label > span {
    width: 70px;
    flex-shrink: 0;
    color: var(--fg-dim);
    font-size: 12px;
  }
  label input:not([type="checkbox"]),
  textarea {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px 10px;
    outline: none;
    color: var(--fg);
    font: inherit;
  }
  textarea {
    font-family: var(--mono, monospace);
    font-size: 12.5px;
    resize: vertical;
  }
  label input:focus,
  textarea:focus {
    border-color: var(--accent);
  }
  .note {
    padding: 4px 14px 6px 94px;
    font-size: 12px;
    line-height: 1.5;
  }
  .warn {
    color: var(--yellow);
  }
  .check {
    padding: 8px 14px;
    font-size: 12.5px;
    cursor: pointer;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px 12px;
    border-top: 1px solid var(--border);
    margin-top: 6px;
  }
  .grow {
    flex: 1;
    font-size: 12px;
  }
  .danger {
    color: var(--red);
  }
</style>
