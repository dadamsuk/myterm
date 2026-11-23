<script lang="ts">
  import type { Session } from "../lib/api";
  import { basename, dotClass, isReachable, lastUsed, relTime, switcherOrder } from "../lib/store.svelte";
  import { activate } from "../lib/actions";
  import Picker from "./Picker.svelte";

  const items = switcherOrder();
</script>

<Picker
  {items}
  text={(s: Session) => `${s.display_name} ${s.cwd} ${s.host}`}
  onpick={activate}
  placeholder="Switch to session…"
  empty="No sessions"
>
  {#snippet row(s: Session)}
    <span class="dot {dotClass(s)}"></span>
    <span class="name" class:greyed={!isReachable(s)}>{s.display_name}</span>
    <span class="tag">{s.host}</span>
    <span class="path faint">{basename(s.cwd)}</span>
    <span class="t faint">{relTime(lastUsed(s))}</span>
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
