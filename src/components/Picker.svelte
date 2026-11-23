<script lang="ts" generics="T">
  import type { Snippet } from "svelte";
  import { fuzzyFilter } from "../lib/fuzzy";
  import { app } from "../lib/store.svelte";

  interface Props {
    items: T[];
    text: (item: T) => string;
    row: Snippet<[T]>;
    onpick: (item: T) => void;
    placeholder?: string;
    empty?: string;
    onback?: () => void;
  }
  let { items, text, row, onpick, placeholder = "", empty = "No matches", onback }: Props = $props();

  let query = $state("");
  let sel = $state(0);
  let input: HTMLInputElement;
  let list: HTMLDivElement;

  let shown = $derived(fuzzyFilter(items, query, text));

  $effect(() => {
    // Reset the selection whenever the query changes.
    query;
    sel = 0;
  });

  $effect(() => {
    input?.focus();
  });

  function key(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
      e.preventDefault();
      sel = Math.min(shown.length - 1, sel + 1);
      scroll();
    } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
      e.preventDefault();
      sel = Math.max(0, sel - 1);
      scroll();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = shown[sel];
      if (item !== undefined) onpick(item);
    } else if (e.key === "Backspace" && query === "" && onback) {
      e.preventDefault();
      onback();
    }
  }

  function scroll() {
    queueMicrotask(() => list?.querySelector(".sel")?.scrollIntoView({ block: "nearest" }));
  }
</script>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (app.overlay = null)}>
  <div class="dialog" role="dialog">
    <input class="query" bind:this={input} bind:value={query} {placeholder} onkeydown={key} spellcheck="false" autocomplete="off" />
    <div class="list" bind:this={list}>
      {#each shown as item, i}
        <div
          class="item"
          class:sel={i === sel}
          role="option"
          aria-selected={i === sel}
          tabindex="-1"
          onpointermove={() => (sel = i)}
          onclick={() => onpick(item)}
          onkeydown={() => {}}
        >
          {@render row(item)}
        </div>
      {:else}
        <div class="empty">{empty}</div>
      {/each}
    </div>
  </div>
</div>
