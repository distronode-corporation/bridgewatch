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
    /** This watch's current failure was acknowledged: the tray draws it a level lower. */
    acknowledged?: boolean;
    /** Acknowledge the failure (`true`) or take it back (`false`). No button without it. */
    onacknowledge?: (watch: string, acknowledged: boolean) => void;
  }

  let { watch, now, expansion, acknowledged = false, onacknowledge }: Props = $props();

  // A secondary watch is one line until asked. That is the whole reason the
  // role exists: an hourly schedule that is red by design must be visible
  // without being loud.
  const secondary = $derived(watch.role === "secondary");
  let open = $state(false);
  const expanded = $derived(!secondary || open);
  // The core resolves the effective mode (`show.jobs`, else `ui.jobs`); a
  // recording made before the key existed has none, which means "all".
  const mode = $derived(watch.jobs ?? "all");
  // Absent means GitLab: the core omits the key for it, so its JSON is unchanged.
  const provider = $derived(watch.provider ?? "gitlab");
  // Only a primary watch's failure colours the tray, so only it can be
  // acknowledged; the core refuses anything else anyway.
  const acknowledgeable = $derived(
    !secondary && (watch.icon_state === "failed" || watch.icon_state === "deployed_with_failure"),
  );
  const TOOL_BUTTON =
    "text-muted-foreground hover:text-foreground hover:bg-background focus-visible:ring-ring/50 text-ui-meta shrink-0 rounded-sm px-1 outline-none focus-visible:ring-2";
</script>

<!-- Watches are set apart by 16px of space above each header (`pt-4`), with
     no tinted band and no rule: the space is the separator. -->
<section class="watch pt-4" data-watch={watch.id} data-role={watch.role}>
  <header class="flex min-w-0 items-center gap-2 px-3 pb-1">
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
    <span class="id text-ui-body min-w-0 truncate font-semibold" title={watch.id}>{watch.id}</span>
    {#if !watch.icon_state && secondary}
      <!-- ⛔ The label is the ROLE, so it is read off the role. Branching on
           `icon_state` labelled a PRIMARY watch "secondary" whenever it had no
           state to show — which is exactly a fresh install whose ref filter
           matched nothing, i.e. the first thing a new user sees. -->
      <span class="role text-muted-foreground text-ui-badge shrink-0">secondary</span>
    {/if}
    <span class="flex-1"></span>
    {#if watch.rows.length === 0 && !watch.error}
      <span class="empty text-muted-foreground text-ui-meta shrink-0">no matching pipelines</span>
    {/if}
    {#if acknowledgeable && onacknowledge}
      <!-- Local only: the tray drops a level until the next new failure. -->
      {#if acknowledged}
        <span class="acknowledged text-muted-foreground text-ui-meta shrink-0" data-slot="acknowledged">acknowledged</span>
      {/if}
      <button
        type="button"
        class={TOOL_BUTTON}
        data-slot="acknowledge"
        aria-pressed={acknowledged}
        aria-label={acknowledged
          ? `Stop acknowledging the failure of ${watch.id}`
          : `Acknowledge the failure of ${watch.id}`}
        title={acknowledged
          ? "Show this failure at full level in the tray again"
          : "Draw the tray a level lower for this failure, until the next new one"}
        onclick={() => onacknowledge(watch.id, !acknowledged)}>{acknowledged ? "undo" : "acknowledge"}</button
      >
    {/if}
    {#if watch.icon_state}
      <!-- The watch's verdict, right-aligned, said once for the watch. -->
      <span
        class={["headline text-ui-meta shrink-0", TONE_TEXT[stateTone(watch.icon_state)]]}
        data-headline={watch.icon_state}>{stateWord(watch.icon_state)}</span
      >
    {/if}
  </header>

  {#if watch.error}
    <div class={["error text-ui-meta mx-3 mt-1 rounded-md px-2 py-1", TONE_TEXT.red, TONE_BG.red]}>{watch.error}</div>
  {/if}

  {#each watch.rows as row, index (row.id)}
    <!-- A collapsed secondary watch still shows its newest row: hiding it
         entirely would leave a header with no information in it at all. -->
    {#if expanded || index === 0}
      <PipelineRow
        {row}
        {now}
        {mode}
        {provider}
        {expansion}
        collapsed={!expanded}
        watchId={watch.id}
        actions={watch.actions ?? false}
        watchState={watch.icon_state}
      />
    {/if}
  {/each}
</section>
