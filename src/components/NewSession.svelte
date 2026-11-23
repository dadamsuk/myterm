<script lang="ts">
  import { api, type HostInfo } from "../lib/api";
  import { fuzzyFilter } from "../lib/fuzzy";
  import { app, basename, knownFolders, type FolderEntry } from "../lib/store.svelte";
  import { createSession } from "../lib/actions";

  let { sessionKind }: { sessionKind: "claude" | "shell" } = $props();

  const usable = (h: HostInfo) => !h.hidden && ((h.kind === "tmux" && h.connected) || h.kind === "ephemeral");
  const hosts = app.hosts.filter(usable);
  const folders = knownFolders();
  const defaultHost = folders.find((f) => hosts.some((h) => h.alias === f.host))?.host ?? hosts[0]?.alias ?? "local";

  let field = $state(0);
  let folder = $state("");
  let host = $state(defaultHost);
  let hostQuery = $state(defaultHost);
  let name = $state("");
  let nameTouched = $state(false);
  let sel = $state(0);
  let completions = $state<FolderEntry[]>([]);
  let busy = $state(false);

  let inputs: HTMLInputElement[] = $state([]);

  const isPathMode = (s: string) => /^[~/]/.test(s) || /^[A-Za-z]:[\\/]/.test(s);
  let pathMode = $derived(isPathMode(folder));

  let folderItems = $derived(
    pathMode ? completions : fuzzyFilter(folders, folder, (f) => `${f.path} ${f.host}`).slice(0, 50),
  );
  let hostItems = $derived(fuzzyFilter(hosts, hostQuery === host ? "" : hostQuery, (h) => h.alias));

  // Live completion on the selected host.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let reqId = 0;
  $effect(() => {
    const prefix = folder;
    const h = host;
    if (!isPathMode(prefix)) return;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const id = ++reqId;
      try {
        const list = await api.completePath(h, prefix);
        if (id === reqId) completions = list.map((path) => ({ host: h, path, t: 0 }));
      } catch {
        if (id === reqId) completions = [];
      }
    }, 120);
  });

  $effect(() => {
    // Selection resets as the list changes; path mode starts with nothing selected.
    folder;
    field;
    hostQuery;
    sel = field === 0 && pathMode ? -1 : 0;
  });

  $effect(() => {
    inputs[field]?.focus();
  });

  $effect(() => {
    if (!nameTouched) name = basename(folder.replace(/[\\/]+$/, "")) || "";
  });

  function pickFolder(f: FolderEntry, advance: boolean) {
    folder = f.path;
    if (hosts.some((h) => h.alias === f.host)) {
      host = f.host;
      hostQuery = f.host;
    }
    if (advance) field = 1;
  }

  function pickHost(h: HostInfo) {
    host = h.alias;
    hostQuery = h.alias;
    field = 2;
  }

  async function submit() {
    if (busy) return;
    const n = name.trim() || basename(folder) || "session";
    if (!hosts.some((h) => h.alias === host)) {
      field = 1;
      return;
    }
    busy = true;
    await createSession(host, folder.trim(), n, sessionKind);
    busy = false;
  }

  function move(delta: number, len: number) {
    if (len === 0) return;
    sel = Math.max(field === 0 && pathMode ? -1 : 0, Math.min(len - 1, sel + delta));
  }

  function onKey(e: KeyboardEvent) {
    const list = field === 0 ? folderItems : field === 1 ? hostItems : [];
    if (e.key === "ArrowDown") {
      e.preventDefault();
      move(1, list.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(-1, list.length);
    } else if (e.key === "Tab" && !e.shiftKey && field === 0 && pathMode && folderItems.length) {
      // Drill into the highlighted (or first) completion.
      e.preventDefault();
      folder = folderItems[Math.max(0, sel)].path;
    } else if (e.key === "Tab") {
      e.preventDefault();
      if (field === 1 && hostItems[sel] && hostQuery !== host) pickHost(hostItems[sel]);
      field = (field + (e.shiftKey ? 2 : 1)) % 3;
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (field === 0) {
        if (sel >= 0 && folderItems[sel]) pickFolder(folderItems[sel], true);
        else field = 1;
      } else if (field === 1) {
        if (hostItems[sel]) pickHost(hostItems[sel]);
      } else {
        submit();
      }
    }
  }

  function hostLabel(h: HostInfo) {
    return h.kind === "ephemeral" ? "ephemeral" : h.os;
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog" aria-label="New session">
    <div class="title">New {sessionKind === "claude" ? "Claude" : "shell"} session</div>
    <label class:on={field === 0}>
      <span>Folder</span>
      <input
        bind:this={inputs[0]}
        bind:value={folder}
        onfocus={() => (field = 0)}
        onkeydown={onKey}
        placeholder="fuzzy search known folders, or type / or ~ for a path"
        spellcheck="false"
        autocomplete="off"
      />
    </label>
    <label class:on={field === 1}>
      <span>Host</span>
      <input
        bind:this={inputs[1]}
        bind:value={hostQuery}
        onfocus={(e) => { field = 1; (e.target as HTMLInputElement).select(); }}
        onkeydown={onKey}
        spellcheck="false"
        autocomplete="off"
      />
    </label>
    <label class:on={field === 2}>
      <span>Name</span>
      <input
        bind:this={inputs[2]}
        bind:value={name}
        oninput={() => (nameTouched = true)}
        onfocus={() => (field = 2)}
        onkeydown={onKey}
        spellcheck="false"
        autocomplete="off"
      />
    </label>

    <div class="list">
      {#if field === 0}
        {#each folderItems as f, i (f.host + f.path)}
          <div class="item" class:sel={i === sel} role="option" aria-selected={i === sel} tabindex="-1"
            onpointermove={() => (sel = i)} onclick={() => pickFolder(f, !pathMode)} onkeydown={() => {}}>
            <span class="path">{f.path}</span>
            <span class="tag">{f.host}</span>
          </div>
        {:else}
          <div class="empty">
            {#if pathMode}No matching folders on {host}{:else if folders.length}No match. Type a path starting with / or ~{:else}Type a path starting with / or ~, or leave empty for the home folder{/if}
          </div>
        {/each}
      {:else if field === 1}
        {#each hostItems as h, i (h.alias)}
          <div class="item" class:sel={i === sel} role="option" aria-selected={i === sel} tabindex="-1"
            onpointermove={() => (sel = i)} onclick={() => pickHost(h)} onkeydown={() => {}}>
            <span class="path">{h.alias}</span>
            {#if h.alias === host}<span class="faint">selected</span>{/if}
            <span class="tag">{hostLabel(h)}</span>
          </div>
        {:else}
          <div class="empty">No reachable host matches</div>
        {/each}
      {:else}
        <div class="hint dim">
          <kbd>Enter</kbd> creates {sessionKind === "claude" ? `claude --rc '${name || "…"}'` : "a shell"} in
          <b>{folder || "~"}</b> on <b>{host}</b>
          {#if hosts.find((h) => h.alias === host)?.kind === "ephemeral"}<br /><span class="warn">Ephemeral: it ends when the app quits or the connection drops.</span>{/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .title {
    padding: 12px 14px 6px;
    font-weight: 600;
  }
  label {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 14px;
  }
  label span {
    width: 52px;
    color: var(--fg-dim);
    font-size: 12px;
  }
  label.on span {
    color: var(--accent);
  }
  label input {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px 10px;
    outline: none;
  }
  label.on input {
    border-color: var(--accent);
  }
  .list {
    margin-top: 8px;
    border-top: 1px solid var(--border);
    min-height: 120px;
  }
  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hint {
    padding: 14px;
    line-height: 1.7;
  }
  .warn {
    color: var(--yellow);
  }
</style>
