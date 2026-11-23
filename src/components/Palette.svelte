<script lang="ts">
  import type { HostInfo } from "../lib/api";
  import {
    activeRow,
    app,
    isMac,
    rowCwd,
    rowName,
    rowOnScreen,
    sessionByKey,
    setListMode,
    toast,
    visibleRows,
    type Row,
  } from "../lib/store.svelte";
  import {
    copyAttachCommand,
    killRow,
    openSettings,
    reconnectHost,
    setHostHidden,
    toggleSidebar,
  } from "../lib/actions";
  import { terminals } from "../lib/terminals";
  import { label } from "../lib/keys";
  import Picker from "./Picker.svelte";

  interface Choice {
    label: string;
    detail?: string;
    keys?: string;
    run: () => void;
  }

  let step = $state<{ title: string; items: Choice[] } | null>(null);

  function pickRow(title: string, act: (r: Row) => void) {
    const list = visibleRows();
    const i = list.findIndex(rowOnScreen);
    if (i > 0) list.unshift(list.splice(i, 1)[0]);
    step = {
      title,
      items: list.map((r) => ({ label: rowName(r), detail: `${r.s.host} · ${rowCwd(r)}`, run: () => act(r) })),
    };
  }

  function pickHost(title: string, hosts: HostInfo[], act: (h: HostInfo) => void) {
    if (hosts.length === 0) {
      app.overlay = null;
      toast("No matching hosts");
      return;
    }
    step = { title, items: hosts.map((h) => ({ label: h.alias, detail: h.status, run: () => act(h) })) };
  }

  const done = () => (app.overlay = null);
  const shown = () => app.hosts.filter((h) => !h.hidden);
  const k = app.config?.keys;
  const mac = isMac();

  const commands: Choice[] = [
    { label: "New Claude session…", keys: k && label(k.new_session, mac), run: () => (app.overlay = { kind: "new", sessionKind: "claude" }) },
    { label: "New shell session…", run: () => (app.overlay = { kind: "new", sessionKind: "shell" }) },
    { label: "Switch session…", keys: k && label(k.switcher, mac), run: () => (app.overlay = { kind: "switcher" }) },
    {
      label: "Rename…",
      detail: "the window on screen; renames the tmux window",
      run: () => {
        const r = activeRow();
        if (r) app.overlay = { kind: "rename", row: r };
        else pickRow("Rename which?", (x) => (app.overlay = { kind: "rename", row: x }));
      },
    },
    { label: "Kill…", run: () => pickRow("Kill which?", (r) => { done(); killRow(r); }) },
    {
      label: "Copy attach command",
      run: () => {
        const s = sessionByKey(app.active);
        done();
        if (s?.persistent) copyAttachCommand(s);
        else toast("No tmux session on screen", true);
      },
    },
    { label: "Group by machine", run: () => { setListMode("machine"); done(); } },
    { label: "Sort by name", run: () => { setListMode("name"); done(); } },
    { label: "Sort by recent", run: () => { setListMode("recent"); done(); } },
    { label: "Reconnect host", run: () => pickHost("Reconnect which host?", shown(), (h) => { done(); reconnectHost(h.alias); }) },
    {
      label: "Log in to host…",
      detail: "interactive ssh, for hosts that need a passphrase or host-key confirmation",
      run: () => pickHost("Log in to which host?", shown().filter((h) => !h.local), (h) => { done(); terminals.openLogin(h.alias); }),
    },
    { label: "Hide host", run: () => pickHost("Hide which host?", shown().filter((h) => !h.local), (h) => { done(); setHostHidden(h.alias, true); }) },
    { label: "Show hidden hosts", run: () => pickHost("Show which host?", app.hosts.filter((h) => h.hidden), (h) => { done(); setHostHidden(h.alias, false); }) },
    {
      label: "Install attention hooks on host…",
      run: () =>
        pickHost(
          "Install hooks on which host?",
          shown().filter((h) => (h.kind === "tmux" && h.connected) || (h.local && h.kind === "ephemeral")),
          (h) => (app.overlay = { kind: "hooks", host: h.alias, install: true }),
        ),
    },
    {
      label: "Uninstall attention hooks from host…",
      run: () =>
        pickHost(
          "Uninstall hooks from which host?",
          shown().filter((h) => h.hooks_installed),
          (h) => (app.overlay = { kind: "hooks", host: h.alias, install: false }),
        ),
    },
    { label: "Larger font", keys: mac ? "Cmd+=" : "Ctrl+=", run: () => { done(); terminals.zoom(1); } },
    { label: "Smaller font", keys: mac ? "Cmd+-" : "Ctrl+-", run: () => { done(); terminals.zoom(-1); } },
    { label: "Reset font size", keys: mac ? "Cmd+0" : "Ctrl+0", run: () => { done(); terminals.zoom(0); } },
    { label: "Toggle sidebar", keys: k && label(k.toggle_sidebar, mac), run: () => { done(); toggleSidebar(); } },
    { label: "Open settings file", run: () => { done(); openSettings(); } },
  ];
</script>

{#key step}
  <Picker
    items={step ? step.items : commands}
    text={(c: Choice) => c.label + " " + (c.detail ?? "")}
    onpick={(c: Choice) => c.run()}
    placeholder={step ? step.title : "Type a command…"}
    onback={step ? () => (step = null) : undefined}
  >
    {#snippet row(c: Choice)}
      <span class="label">{c.label}</span>
      {#if c.detail}<span class="detail faint">{c.detail}</span>{/if}
      {#if c.keys}<kbd class="keys">{c.keys}</kbd>{/if}
    {/snippet}
  </Picker>
{/key}

<style>
  .label {
    flex-shrink: 0;
  }
  .detail {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
  }
  .keys {
    margin-left: auto;
  }
</style>
