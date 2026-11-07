<script lang="ts">
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import ExternalLinkIcon from "@lucide/svelte/icons/external-link";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import { untrack } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { TONE_DOT, TONE_TEXT, errorLines, isFirstRun, stateTone, stateWord } from "../lib/format";
  import type { Snapshot, Status } from "../lib/types";
  import CrashReportsAsk from "./CrashReportsAsk.svelte";
  import DebugPane from "./DebugPane.svelte";
  import ErrorStrip from "./ErrorStrip.svelte";
  import NeedsApprovalStrip from "./NeedsApprovalStrip.svelte";
  import NotificationsPaused from "./NotificationsPaused.svelte";
  import TokenHealthStrip from "./TokenHealthStrip.svelte";
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
    /** One line saying why the last request did not happen, or empty. */
    notice?: string;
    onrefresh?: () => void;
    onsettings?: () => void;
    onpipelines?: () => void;
    onquit?: () => void;
    /** Open Settings on the broken file (the error strip's button). */
    onfix?: () => void;
    /** Open the setup wizard (the error strip's button on a first launch). */
    onsetup?: () => void;
    /**
     * Put the crash-report question. The caller decides (from `Status`), and
     * answers it with `oncrashreports`.
     */
    askCrashReports?: boolean;
    /** The crash-report answer: `true` is Send. */
    oncrashreports?: (send: boolean) => void;
    /** End a notification pause (the paused strip's Resume). */
    onresume?: () => void;
  }

  let {
    snapshot,
    status = null,
    now = Date.now(),
    expansion = createExpansionStore(),
    notice = "",
    onrefresh,
    onsettings,
    onpipelines,
    onquit,
    onfix,
    onsetup,
    askCrashReports = false,
    oncrashreports,
    onresume,
  }: Props = $props();

  // Configuration order, primary first. The core already returns watches in
  // configuration order, so this is a stable partition of that and never a sort.
  const ordered = $derived([
    ...snapshot.watches.filter((w) => w.role === "primary"),
    ...snapshot.watches.filter((w) => w.role !== "primary"),
  ]);

  /**
   * The strip's errors: the snapshot's, less the ones a watch section below
   * already shows. ⛔ The core lists each watch's error in `errors` too, as
   * `<watch id>: <error>`, for `check` and the CLI; shown in both places, a
   * refused account's reason (a host move, a cancelled prompt) appeared in the
   * strip AND under every one of its watches.
   */
  const stripErrors = $derived.by(() => {
    const shown = new Set(snapshot.watches.filter((w) => w.error).map((w) => `${w.id}: ${w.error}`));
    return snapshot.errors.filter((e) => !shown.has(e));
  });

  // The strip's lines, Status included, so the empty-list message below agrees
  // with what the strip says.
  const hasErrors = $derived(errorLines(stripErrors, status).length > 0);

  // Forget open/closed choices for pipelines that have left the popover, so the
  // store does not grow for the life of the process.
  $effect(() => {
    const ids = snapshot.watches.flatMap((w) => w.rows.map((r) => r.id));
    // Untracked: pruning reads the store, and a user's toggle must not re-run it.
    untrack(() => expansion.prune(ids));
  });

  const tone = $derived(stateTone(snapshot.icon_state));

  /** The Debug pane, opened from the footer and shown just above it. */
  let debugOpen = $state(false);
</script>

<div
  class="popover bg-background text-foreground border-border text-ui-body flex h-screen flex-col overflow-hidden rounded-xl border"
  data-popover
>
  <!-- The header is one line: the state, a dot and "Ns ago", then two icon
       buttons. ⛔ Icon-only, so each keeps the accessible name its text label
       used to give it (`aria-label`), and a tooltip says what it does. -->
  <header class="border-border flex items-center gap-2 border-b py-2 pr-2 pl-3">
    <span class={["size-2.5 shrink-0 rounded-full", TONE_DOT[tone]]} aria-hidden="true"></span>
    <span class="flex min-w-0 items-baseline gap-1">
      <span class={["headline truncate font-semibold", TONE_TEXT[tone]]} data-icon-state={snapshot.icon_state}>
        {stateWord(snapshot.icon_state)}
      </span>
      <span class="text-muted-foreground text-ui-meta shrink-0" aria-hidden="true">·</span>
      <UpdatedAgo at={snapshot.last_poll} {now} short class="shrink-0 whitespace-nowrap" />
    </span>
    <span class="flex-1"></span>
    <Button variant="ghost" size="icon-xs" onclick={onrefresh} title="Poll now" aria-label="Refresh" data-action="refresh">
      <RefreshCwIcon class="size-3.5" aria-hidden="true" />
    </Button>
    <Button
      variant="ghost"
      size="icon-xs"
      onclick={onpipelines}
      title="Open the pipelines page"
      aria-label="Pipelines"
      data-action="pipelines"
    >
      <ExternalLinkIcon class="size-3.5" aria-hidden="true" />
    </Button>
  </header>

  {#if notice}
    <p class="notice text-tone-amber border-border m-0 border-b px-3 py-1 text-xs" role="status" data-slot="popover-notice">
      {notice}
    </p>
  {/if}

  <ErrorStrip errors={stripErrors} {status} {onfix} {onsetup} />

  <NeedsApprovalStrip accounts={status?.needsApproval ?? []} onretry={onrefresh} />

  <TokenHealthStrip tokens={snapshot.tokens ?? []} {onsettings} />

  <NotificationsPaused paused={status?.notificationsPaused ?? null} {onresume} />

  {#if askCrashReports && oncrashreports}
    <CrashReportsAsk onanswer={oncrashreports} />
  {/if}

  <!-- ⛔ `min-h-0` is load-bearing. A flex item's default `min-height: auto`
       refuses to shrink below its content, so without it the list pushes the
       debug pane off the bottom of the window instead of scrolling, and the
       height this component reports back is never the one it needs. -->
  <div class="scroll min-h-0 flex-1 overflow-y-auto pb-3" data-scroll>
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

  <DebugPane {snapshot} {status} open={debugOpen} id="popover-debug" />

  <!-- The footer: what is used rarely, out of the header. -->
  <footer class="border-border text-ui-meta flex items-center gap-2 border-t py-1 pr-2 pl-1.5" data-slot="popover-footer">
    <button
      type="button"
      class="debug-toggle text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 flex h-6 items-center gap-0.5 rounded-md px-1.5 outline-none focus-visible:ring-2"
      aria-expanded={debugOpen}
      aria-controls="popover-debug"
      onclick={() => (debugOpen = !debugOpen)}
    >
      Debug
      <ChevronRightIcon class={["size-3 transition-transform", debugOpen && "-rotate-90"]} aria-hidden="true" />
    </button>
    <span class="flex-1"></span>
    <Button variant="ghost" size="xs" onclick={onsettings} title="Settings">Settings…</Button>
    <Button variant="ghost" size="xs" onclick={onquit} title="Quit bridgewatch">Quit</Button>
  </footer>
</div>
