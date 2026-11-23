<script lang="ts">
  import {
    basename,
    isReachable,
    multiSession,
    relTime,
    rowCwd,
    rowDot,
    rowLastUsed,
    rowName,
    switcherOrder,
    type Row,
  } from "../lib/store.svelte";
  import { activateRow } from "../lib/actions";
  import Picker from "./Picker.svelte";

  const items = switcherOrder();
</script>

<Picker
  {items}
  text={(r: Row) => `${rowName(r)} ${rowCwd(r)} ${r.s.host} ${r.s.display_name}`}
  onpick={activateRow}
  placeholder="Switch to…"
  empty="No sessions"
>
  {#snippet row(r: Row)}
    <span class="dot {rowDot(r)}"></span>
    <span class="name" class:greyed={!isReachable(r.s)}>{rowName(r)}</span>
    <span class="tag">{r.s.host}</span>
    {#if r.s.persistent && multiSession(r.s.host)}<span class="faint sess">{r.s.display_name}</span>{/if}
    <span class="path faint">{basename(rowCwd(r))}</span>
    <span class="t faint">{relTime(rowLastUsed(r))}</span>
  {/snippet}
</Picker>

<style>
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .greyed {
    opacity: 0.5;
  }
  .sess {
    font-size: 11px;
  }
  .path {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
  }
  .t {
    font-size: 11px;
  }
</style>
