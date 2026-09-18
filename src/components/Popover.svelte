<script lang="ts">
  import { untrack } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { TONE_DOT, TONE_TEXT, errorLines, isFirstRun, stateTone, stateWord } from "../lib/format";
  import type { Snapshot, Status } from "../lib/types";
  import DebugPane from "./DebugPane.svelte";
  import ErrorStrip from "./ErrorStrip.svelte";
  import { UpdatedAgo, createExpansionStore, type ExpansionStore } from "./jobs";
  import WatchSection from "./WatchSection.svelte";

  interface Props {
    snapshot: Snapshot;
    status?: Status | null;
    /** One clock per frame; the caller decides when it moves. */
    now?: number;
    /**
     * Which bridges the user opened or closed. One per window, created here by
     * default; a snapshot refresh replaces every row object and must not
     * collapse what the user opened.
     */
    expansion?: ExpansionStore;
    onrefresh?: () => void;
    onsettings?: () => void;
    onpipelines?: () => void;
    onquit?: () => void;
    /** Open Settings on the broken file (the error strip's button). */
    onfix?: () => void;
    /** Open the setup wizard (the error strip's button on a first launch). */
    onsetup?: () => void;
  }

  let {
    snapshot,
    status = null,
    now = Date.now(),
    expansion = createExpansionStore(),
    onrefresh,
    onsettings,
    onpipelines,
    onquit,
    onfix,
    onsetup,
  }: Props = $props();

  // Configuration order, primary first. The core already returns watches in
  // configuration order, so this is a stable partition of that and never a sort.
  const ordered = $derived([
    ...snapshot.watches.filter((w) => w.role === "primary"),
    ...snapshot.watches.filter((w) => w.role !== "primary"),
  ]);

  // The strip's lines, Status included, so the empty-list message below agrees
  // with what the strip says.
  const hasErrors = $derived(errorLines(snapshot.errors, status).length > 0);

  // Forget open/closed choices for pipelines that have left the popover, so the
  // store does not grow for the life of the process.
  $effect(() => {
    const ids = snapshot.watches.flatMap((w) => w.rows.map((r) => r.id));
    // Untracked: pruning reads the store, and a user's toggle must not re-run it.
    untrack(() => expansion.prune(ids));
  });

  const tone = $derived(stateTone(snapshot.icon_state));
</script>

<div
  class="popover bg-background text-foreground border-border flex h-screen flex-col overflow-hidden rounded-xl border text-[13px]"
  data-popover
>
  <header class="bg-muted/60 border-border flex items-center gap-1.5 border-b py-1.5 pr-1.5 pl-3">
    <span class={["size-2.5 shrink-0 rounded-full", TONE_DOT[tone]]} aria-hidden="true"></span>
    <span class={["headline font-semibold", TONE_TEXT[tone]]} data-icon-state={snapshot.icon_state}>
      {stateWord(snapshot.icon_state)}
    </span>
    <UpdatedAgo at={snapshot.last_poll} {now} class="ml-1 text-[11px]" />
    <span class="flex-1"></span>
    <Button variant="ghost" size="xs" onclick={onrefresh} title="Poll now">Refresh</Button>
    <Button variant="ghost" size="xs" onclick={onpipelines} title="Open the pipelines page">Pipelines</Button>
    <Button variant="ghost" size="xs" onclick={onsettings} title="Settings">Settings</Button>
    <Button variant="ghost" size="xs" onclick={onquit} title="Quit bridgewatch">Quit</Button>
  </header>

  <ErrorStrip errors={snapshot.errors} {status} {onfix} {onsetup} />

  <!-- ⛔ `min-h-0` is load-bearing. A flex item's default `min-height: auto`
       refuses to shrink below its content, so without it the list pushes the
       debug pane off the bottom of the window instead of scrolling, and the
       height this component reports back is never the one it needs. -->
  <div class="scroll min-h-0 flex-1 overflow-y-auto" data-scroll>
    {#if ordered.length === 0 && isFirstRun(status)}
      <p class="empty text-muted-foreground m-0 px-3 py-4">Nothing is watched yet. The setup wizard gets the basics in place.</p>
    {:else if ordered.length === 0 && hasErrors}
      <!-- ⛔ Not "No watches configured". A configuration that fails to load
           produces a snapshot with no watches AND an error, and telling the
           user to go and add a watch sends them to fix the wrong thing — the
           watches are very likely already in the file the error is about. -->
      <p class="empty text-muted-foreground m-0 px-3 py-4">Nothing to show until the problem above is fixed.</p>
    {:else if ordered.length === 0}
      <p class="empty text-muted-foreground m-0 px-3 py-4">
        No watches configured. Settings → Watches, or edit the file directly.
      </p>
    {:else}
      {#each ordered as watch (watch.id)}
        <WatchSection {watch} {now} {expansion} />
      {/each}
    {/if}
  </div>

  <DebugPane {snapshot} {status} />
</div>
