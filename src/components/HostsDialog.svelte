<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { api, type HostEntry } from "../lib/api";
  import { app, hostInfo, toast } from "../lib/store.svelte";
  import { hostAction, hostEntries, openSshConfig, saveHosts } from "../lib/actions";
  import { hostColor, hostStatus } from "../lib/hostdot";

  // Edited here and written through on every change; the snapshot that
  // follows carries the new order back into app.hosts.
  let { focusHost }: { focusHost?: string } = $props();

  let list = $state<HostEntry[]>(untrack(hostEntries));
  const android = app.platform === "android";
  // Built-in client hosts (spec 14.4) have their address, user and port here.
  let anyBuiltin = $derived(android || app.hosts.some((h) => h.builtin));
  type Det = { address: string; user: string; port: string };
  const blank = (): Det => ({ address: "", user: "", port: "" });
  function details(alias: string): Det {
    const o = app.config?.hosts?.[alias];
    return { address: o?.address ?? "", user: o?.user ?? "", port: o?.port ? String(o.port) : "" };
  }
  let editing = $state<string | null>(untrack(() => focusHost ?? null));
  let det = $state<Det>(untrack(() => (focusHost ? details(focusHost) : blank())));
  let newDet = $state<Det>(blank());
  let publicKey = $state("");

  function toggleEdit(alias: string) {
    editing = editing === alias ? null : alias;
    if (editing) det = details(alias);
  }

  const portOf = (s: string) => {
    const n = parseInt(s.trim(), 10);
    return n > 0 && n < 65536 ? n : null;
  };
  async function saveDetails(alias: string, d: Det) {
    try {
      await api.setHostDetails(alias, d.address.trim() || null, d.user.trim() || null, portOf(d.port));
      return true;
    } catch (e) {
      toast(String(e), true);
      return false;
    }
  }
  async function saveEdit(alias: string) {
    if (await saveDetails(alias, det)) {
      editing = null;
      toast(`Saved; reconnecting ${alias}`);
    }
  }
  async function forgetKey(alias: string) {
    try {
      await api.forgetHostKey(alias);
      toast(`Forgot the host key of ${alias}; the next connection trusts the key it offers`);
    } catch (e) {
      toast(String(e), true);
    }
  }
  async function copyKey() {
    try {
      await writeText(publicKey);
      // Android confirms a copy itself.
      if (!android) toast("Public key copied");
    } catch (e) {
      toast(String(e), true);
    }
  }
  let aliases = $state<string[]>([]);
  let name = $state("");
  let autoConnect = $state(true);
  let input: HTMLInputElement;

  onMount(() => {
    // On a phone, focusing would open the keyboard over the list.
    if (!app.phone) input.focus();
    api.sshAliases().then((a) => (aliases = a)).catch(() => {});
    if (untrack(() => anyBuiltin)) api.builtinPublicKey().then((k) => (publicKey = k)).catch(() => {});
  });

  let suggestions = $derived(
    [...(android ? [] : ["local"]), ...aliases].filter((a) => !list.some((h) => h.name === a)),
  );

  function commit(next: HostEntry[]) {
    list = next;
    saveHosts($state.snapshot(next));
  }

  async function add() {
    const n = name.trim();
    if (!n) return;
    if (!list.some((h) => h.name === n)) {
      // Details first, so the host's first connection uses them.
      if (android && (newDet.address.trim() || newDet.user.trim() || newDet.port.trim())) {
        if (!(await saveDetails(n, newDet))) return;
      }
      commit([...list, { name: n, auto_connect: autoConnect }]);
    }
    name = "";
    newDet = blank();
    if (!app.phone) input.focus();
  }

  function move(i: number, d: number) {
    const j = i + d;
    if (j < 0 || j >= list.length) return;
    const next = [...list];
    [next[i], next[j]] = [next[j], next[i]];
    commit(next);
  }

  function setAuto(i: number, on: boolean) {
    commit(list.map((h, k) => (k === i ? { ...h, auto_connect: on } : h)));
  }

  function remove(i: number) {
    commit(list.filter((_, k) => k !== i));
  }

  // Drag to reorder by the handle. Pointer events rather than HTML drag and
  // drop, which the webview's file-drop handling takes over.
  let dragFrom = $state<number | null>(null);
  let rowsEl: HTMLDivElement;
  function dragStart(e: PointerEvent, i: number) {
    dragFrom = i;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function dragMove(e: PointerEvent) {
    if (dragFrom === null) return;
    const rows = [...rowsEl.querySelectorAll<HTMLElement>(".hrow")];
    const to = rows.findIndex((r) => {
      const b = r.getBoundingClientRect();
      return e.clientY >= b.top && e.clientY < b.bottom;
    });
    if (to < 0 || to === dragFrom) return;
    const next = [...list];
    const [x] = next.splice(dragFrom, 1);
    next.splice(to, 0, x);
    list = next;
    dragFrom = to;
  }
  function dragEnd() {
    if (dragFrom === null) return;
    dragFrom = null;
    commit(list);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" class:phone={app.phone} role="dialog" aria-label="Hosts">
    <div class="head">
      <span class="title">Hosts</span>
      <span class="faint">shown in this order</span>
      {#if app.phone}
        <span class="grow"></span>
        <button class="btn" onclick={() => (app.overlay = null)}>Done</button>
      {/if}
    </div>

    <div class="rows" bind:this={rowsEl}>
      {#each list as h, i (h.name)}
        {@const info = hostInfo(h.name)}
        <div class="hrow" class:dragging={dragFrom === i}>
          <span
            class="grip"
            title="Drag to reorder"
            role="button"
            tabindex="-1"
            onpointerdown={(e) => dragStart(e, i)}
            onpointermove={dragMove}
            onpointerup={dragEnd}
            onpointercancel={dragEnd}
          >⋮⋮</span>
          <span class="hdot {info ? hostColor(info) : 'connecting'}"></span>
          <span class="name">{h.name}</span>
          {#if info}
            <button class="st" title={info.status} onclick={() => hostAction(info)}>{hostStatus(info)}</button>
          {:else}
            <span class="st faint">adding…</span>
          {/if}
          <label class="auto" title="Connect when myterm starts. Off: connect on first click.">
            <input type="checkbox" checked={h.auto_connect} onchange={(e) => setAuto(i, e.currentTarget.checked)} />
            auto-connect
          </label>
          <button class="icon" title="Move up" aria-label="Move up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
          <button class="icon" title="Move down" aria-label="Move down" disabled={i === list.length - 1} onclick={() => move(i, 1)}>↓</button>
          {#if info?.builtin || (android && !info)}
            <button
              class="icon"
              class:on={editing === h.name}
              title="Address, user and port"
              aria-label="Edit {h.name}"
              onclick={() => toggleEdit(h.name)}>✎</button
            >
          {/if}
          <button class="icon x" title="Remove from the list" aria-label="Remove" onclick={() => remove(i)}>×</button>
        </div>
        {#if editing === h.name}
          <div class="details">
            <label>Address <input bind:value={det.address} placeholder={android ? "100.x.y.z or name.ts.net" : "from ~/.ssh/config"} spellcheck="false" autocomplete="off" autocapitalize="none" /></label>
            <label>User <input bind:value={det.user} placeholder={android ? "required" : "from ~/.ssh/config"} spellcheck="false" autocomplete="off" autocapitalize="none" /></label>
            <label class="port">Port <input bind:value={det.port} placeholder="22" inputmode="numeric" /></label>
            <div class="dbtns">
              <button class="btn" title="After a host is reinstalled: trust the key it offers next" onclick={() => forgetKey(h.name)}>Forget host key</button>
              <span class="grow"></span>
              <button class="btn" onclick={() => (editing = null)}>Cancel</button>
              <button class="btn primary" onclick={() => saveEdit(h.name)}>Save</button>
            </div>
          </div>
        {/if}
      {:else}
        <div class="empty">No hosts. Add one below.</div>
      {/each}
    </div>

    <div class="add">
      <input
        bind:this={input}
        bind:value={name}
        onkeydown={key}
        list="ssh-aliases"
        placeholder={android ? "Name, e.g. intra" : "Host: an ssh config alias such as intra, or user@server"}
        autocapitalize="none"
        spellcheck="false"
        autocomplete="off"
      />
      <datalist id="ssh-aliases">
        {#each suggestions as a (a)}<option value={a}></option>{/each}
      </datalist>
      <label class="auto" title="Connect when myterm starts">
        <input type="checkbox" bind:checked={autoConnect} />
        auto-connect
      </label>
      <button class="btn primary" disabled={!name.trim()} onclick={add}>Add</button>
    </div>
    {#if android}
      <div class="details newdet">
        <label>Address <input bind:value={newDet.address} placeholder="100.x.y.z or name.ts.net" spellcheck="false" autocomplete="off" autocapitalize="none" /></label>
        <label>User <input bind:value={newDet.user} placeholder="e.g. admin" spellcheck="false" autocomplete="off" autocapitalize="none" /></label>
        <label class="port">Port <input bind:value={newDet.port} placeholder="22" inputmode="numeric" /></label>
      </div>
    {/if}
    {#if suggestions.length}
      <div class="chips">
        <span class="faint">From ~/.ssh/config:</span>
        {#each suggestions as a (a)}
          <button class="chip" onclick={() => ((name = a), add())}>{a}</button>
        {/each}
      </div>
    {/if}
    {#if anyBuiltin && publicKey}
      <div class="key">
        <div class="khead">
          <span>{android ? "This phone's key" : "myterm's own key"}</span>
          <button class="btn" onclick={copyKey}>Copy</button>
        </div>
        <code class="ktext">{publicKey}</code>
        <div class="faint">
          Add it to <code>~/.ssh/authorized_keys</code> on each host. On Windows, for an administrator:
          <code>C:\ProgramData\ssh\administrators_authorized_keys</code>.
        </div>
      </div>
    {/if}
    <div class="foot faint">
      {#if android}
        Saved to config.toml in the app's data folder.
      {:else}
        Saved to config.toml as <code>[[host]]</code> entries, which you can also edit by hand.
        Connection details (HostName, User, keys) stay in
        <button class="link" onclick={openSshConfig}>~/.ssh/config</button>.
      {/if}
    </div>
  </div>
</div>

<style>
  .dialog {
    width: min(680px, 94vw);
  }
  /* Phone: one full-screen page that scrolls as a whole. */
  .dialog.phone {
    position: fixed;
    inset: 0;
    width: auto;
    max-height: none;
    border-radius: 0;
    border: 0;
    overflow-y: auto;
  }
  .dialog.phone .head {
    align-items: center;
  }
  .dialog.phone .rows {
    overflow: visible;
    flex-shrink: 0;
  }
  .dialog.phone .details label:first-child {
    flex-basis: 100%;
  }
  /* Phone: the status goes on a line of its own under the name. */
  .dialog.phone .hrow {
    flex-wrap: wrap;
    row-gap: 0;
    padding: 6px 8px;
  }
  .dialog.phone .name {
    flex: 1;
  }
  .dialog.phone .st {
    order: 10;
    flex-basis: 100%;
    padding-left: 34px;
  }
  .dialog.phone .icon {
    padding: 6px 9px;
    font-size: 16px;
  }
  .icon.on {
    background: var(--selected);
    color: var(--fg);
  }
  .details {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    padding: 6px 10px 10px 34px;
  }
  .details.newdet {
    padding: 0 14px 10px;
  }
  .details label {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 11.5px;
    color: var(--fg-dim);
    flex: 1 1 140px;
    min-width: 0;
  }
  .details label.port {
    flex: 0 0 64px;
  }
  .details input {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 5px 8px;
    outline: none;
    color: var(--fg);
    width: 100%;
  }
  /* 16 px stops the phone zooming in on a focused field. */
  .dialog.phone input:not([type]) {
    font-size: 16px;
  }
  .details input:focus {
    border-color: var(--accent);
  }
  .dbtns {
    display: flex;
    gap: 8px;
    flex-basis: 100%;
  }
  .grow {
    flex: 1;
  }
  .key {
    padding: 10px 14px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    line-height: 1.5;
  }
  .khead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 4px;
    font-weight: 600;
  }
  .ktext {
    display: block;
    word-break: break-all;
    user-select: all;
    background: var(--bg);
    border-radius: 6px;
    padding: 6px 8px;
    margin-bottom: 6px;
    font-size: 11px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 12px 14px 8px;
  }
  .title {
    font-size: 14px;
    font-weight: 600;
  }
  .rows {
    overflow-y: auto;
    padding: 0 6px 6px;
    min-height: 40px;
  }
  .hrow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-radius: 6px;
  }
  .hrow:hover {
    background: var(--hover);
  }
  .hrow.dragging {
    background: var(--selected);
  }
  .grip {
    cursor: grab;
    color: var(--fg-faint);
    font-size: 11px;
    letter-spacing: -2px;
    padding: 0 2px;
    touch-action: none;
  }
  .hrow.dragging .grip {
    cursor: grabbing;
  }
  .name {
    flex-shrink: 0;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    padding: 0;
    text-align: left;
    color: var(--fg-faint);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .st:hover {
    color: var(--fg);
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--fg-dim);
    flex-shrink: 0;
    cursor: pointer;
  }
  .icon {
    background: none;
    border: 0;
    color: var(--fg-dim);
    cursor: pointer;
    padding: 2px 5px;
    border-radius: 4px;
    font-size: 13px;
  }
  .icon:hover:not(:disabled) {
    background: var(--panel-2);
    color: var(--fg);
  }
  .icon:disabled {
    opacity: 0.25;
    cursor: default;
  }
  .icon.x {
    font-size: 15px;
  }
  .add {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
  }
  .add input:not([type]) {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 9px;
    outline: none;
  }
  .add input:not([type]):focus {
    border-color: var(--accent);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    align-items: center;
    padding: 0 14px 10px;
    font-size: 12px;
  }
  .chip {
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1px 9px;
    font-size: 12px;
    cursor: pointer;
  }
  .chip:hover {
    background: var(--hover);
  }
  .foot {
    padding: 8px 14px 12px;
    font-size: 12px;
    line-height: 1.5;
    border-top: 1px solid var(--border);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: inherit;
  }
  .link:hover {
    text-decoration: underline;
  }
  code {
    font-size: 11.5px;
  }
</style>
