<script lang="ts">
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";

  import { TONE_BG, TONE_TEXT, stateTone, stateWord } from "../lib/format";
  import type { WatchView } from "../lib/types";
  import type { ExpansionStore } from "./jobs";
  import PipelineRow from "./PipelineRow.svelte";

  interface Props {
    watch: WatchView;
    now: number;
    /** Bridge open/closed choices, shared by every row in the window. */
    expansion: ExpansionStore;
  }

  let { watch, now, expansion }: Props = $props();

  // A secondary watch is one line until asked. That is the whole reason the
  // role exists: an hourly schedule that is red by design must be visible
  // without being loud.
  const secondary = $derived(watch.role === "secondary");
  let open = $state(false);
  const expanded = $derived(!secondary || open);
  // The core resolves the effective mode (`show.jobs`, else `ui.jobs`); a
  // recording made before the key existed has none, which means "all".
  const mode = $derived(watch.jobs ?? "all");
</script>

<section class="watch border-border border-b last:border-b-0" data-watch={watch.id} data-role={watch.role}>
  <header class="bg-muted/60 flex items-center gap-2 px-3 pt-1.5 pb-1">
    {#if secondary}
      <button
        class="disclosure text-muted-foreground hover:text-foreground -ml-1 rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        aria-expanded={open}
        aria-label={open ? "collapse" : "expand"}
        onclick={() => (open = !open)}
        title={open ? "collapse" : "expand"}
      >
        <ChevronRightIcon class={["size-3.5 transition-transform", open && "rotate-90"]} aria-hidden="true" />
      </button>
    {/if}
    <span class="id text-[13px] font-semibold">{watch.id}</span>
    {#if watch.icon_state}
      <span class={["headline text-xs", TONE_TEXT[stateTone(watch.icon_state)]]} data-headline={watch.icon_state}
        >{stateWord(watch.icon_state)}</span
      >
    {:else if secondary}
      <!-- ⛔ The label is the ROLE, so it is read off the role. Branching on
           `icon_state` labelled a PRIMARY watch "secondary" whenever it had no
           state to show — which is exactly a fresh install whose ref filter
           matched nothing, i.e. the first thing a new user sees. -->
      <span class="role text-muted-foreground text-[11px]">secondary</span>
    {/if}
    <span class="flex-1"></span>
    {#if watch.rows.length === 0 && !watch.error}
      <span class="empty text-muted-foreground text-[11px]">no matching pipelines</span>
    {/if}
  </header>

  {#if watch.error}
    <div class={["error px-3 py-1 text-xs", TONE_TEXT.red, TONE_BG.red]}>{watch.error}</div>
  {/if}

  {#each watch.rows as row, index (row.id)}
    <!-- A collapsed secondary watch still shows its newest row: hiding it
         entirely would leave a header with no information in it at all. -->
    {#if expanded || index === 0}
      <PipelineRow {row} {now} {mode} {expansion} collapsed={!expanded} />
    {/if}
  {/each}
</section>
