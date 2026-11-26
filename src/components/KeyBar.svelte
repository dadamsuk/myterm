<script lang="ts">
  // Phone keys row (spec 14.4): keys a soft keyboard lacks. Ctrl and Alt are
  // sticky for one key, typed or from this row.
  import { app } from "../lib/store.svelte";
  import { terminals } from "../lib/terminals";

  type Key = { label: string; data?: string; mod?: "ctrl" | "alt"; title?: string };
  const keys: Key[] = [
    { label: "Esc", data: "\x1b" },
    { label: "Tab", data: "\t" },
    { label: "⇧Tab", data: "\x1b[Z", title: "Shift+Tab" },
    { label: "Ctrl", mod: "ctrl" },
    { label: "Alt", mod: "alt" },
    { label: "←", data: "\x1b[D" },
    { label: "↑", data: "\x1b[A" },
    { label: "↓", data: "\x1b[B" },
    { label: "→", data: "\x1b[C" },
    { label: "~", data: "~" },
    { label: "|", data: "|" },
    { label: "/", data: "/" },
    { label: "-", data: "-" },
  ];

  function press(e: PointerEvent, k: Key) {
    // Keep focus (and the soft keyboard) on the terminal.
    e.preventDefault();
    if (k.mod) app.keyMods[k.mod] = !app.keyMods[k.mod];
    else if (k.data) terminals.sendKey(k.data);
  }
</script>

<div class="keybar">
  {#each keys as k}
    <button
      class:on={k.mod && app.keyMods[k.mod]}
      title={k.title ?? k.label}
      aria-label={k.title ?? k.label}
      onpointerdown={(e) => press(e, k)}>{k.label}</button
    >
  {/each}
</div>

<style>
  .keybar {
    flex-shrink: 0;
    display: flex;
    overflow-x: auto;
    gap: 4px;
    padding: 5px 6px;
    background: var(--panel);
    border-top: 1px solid var(--border);
    scrollbar-width: none;
  }
  button {
    flex: 1 0 auto;
    min-width: 40px;
    height: 36px;
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--panel-2);
    color: var(--fg);
    font-size: 14px;
    touch-action: manipulation;
    user-select: none;
  }
  button.on {
    background: var(--accent, #3d7eff);
    border-color: transparent;
    color: #fff;
  }
</style>
