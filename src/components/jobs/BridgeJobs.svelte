<script lang="ts">
  /**
   * One bridge (trigger job) as a collapsible row: the verdict on the header,
   * the child pipeline's jobs by stage inside, and on the header two links out:
   * the trigger job itself and the child pipeline it created. They are
   * different pages and either can be the one that explains a verdict.
   *
   * Open/closed lives in the `expansion` store, keyed by pipeline id + bridge
   * name, so a snapshot refresh that replaces `bridge` with a new object does
   * not collapse it.
   */
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";

  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import { bridgeTone, bridgeWord } from "../../lib/format";
  import type { BridgeView, JobsMode } from "../../lib/types";
  import JobList from "./JobList.svelte";
  import { TONE_DOT, TONE_TEXT, filterJobs, type ExpansionStore } from "./jobs";

  interface Props {
    bridge: BridgeView;
    pipelineId: number;
    expansion: ExpansionStore;
    /** Open state for a bridge the user never toggled. */
    defaultOpen?: boolean;
    mode?: JobsMode;
    onOpen: (url: string) => void;
    /** See JobList: an external clock, or omit to let the list tick itself. */
    now?: number;
  }

  let { bridge, pipelineId, expansion, defaultOpen = false, mode = "all", onOpen, now }: Props = $props();

  const open = $derived(expansion.isOpen(pipelineId, bridge.name, defaultOpen));
  const tone = $derived(bridgeTone(bridge.verdict));
  const shownCount = $derived(filterJobs(bridge.jobs, mode).length);
  const bodyId = $derived(`bridge-${pipelineId}-${bridge.name.replace(/[^A-Za-z0-9_-]/g, "_")}`);

  function openUrl(event: MouseEvent, url: string | null) {
    event.preventDefault();
    event.stopPropagation();
    if (url) onOpen(url);
  }
</script>

<Collapsible.Root
  open={open}
  onOpenChange={(next) => expansion.set(pipelineId, bridge.name, next)}
  class="flex flex-col"
  data-bridge={bridge.name}
>
  <div class="hover:bg-muted flex h-7 items-center gap-1.5 rounded-lg pr-1 text-xs">
    <Collapsible.Trigger
      class="focus-visible:ring-ring/50 flex min-w-0 flex-1 items-center gap-1.5 rounded-lg py-1 pl-1 text-left outline-none focus-visible:ring-2"
      aria-controls={bodyId}
      aria-label={`${bridge.name}: ${bridgeWord(bridge.verdict)}. ${open ? "Hide" : "Show"} jobs`}
    >
      <ChevronRightIcon
        class={["text-muted-foreground size-3.5 shrink-0 transition-transform", open && "rotate-90"]}
        aria-hidden="true"
      />
      <span
        class={["size-2 shrink-0 rounded-full", TONE_DOT[tone], bridge.verdict === "running" && "bw-live"]}
        aria-hidden="true"
      ></span>
      <span class="text-foreground min-w-0 truncate font-mono">{bridge.name}</span>
      <span class={["shrink-0", TONE_TEXT[tone]]}>{bridgeWord(bridge.verdict)}</span>
      {#if bridge.dived && bridge.jobs.length > 0}
        <span class="text-muted-foreground shrink-0 tabular-nums" title="jobs listed / jobs in the child">
          {mode === "all" ? bridge.jobs.length : `${shownCount}/${bridge.jobs.length}`}
        </span>
      {/if}
    </Collapsible.Trigger>
    <!-- ⛔ Both links are SIBLINGS of the trigger, never inside it: the bridge
         name sits in a button, and an anchor nested in a button is neither
         valid nor reachable by keyboard. A bridge with no `web_url` gets no
         link rather than a dead one; its name is already plain text above. -->
    {#if bridge.web_url}
      <a
        href={bridge.web_url}
        class="text-muted-foreground focus-visible:ring-ring/50 shrink-0 rounded-sm text-[11px] outline-none hover:underline focus-visible:ring-2"
        title="the trigger job"
        aria-label={`Open trigger job ${bridge.name} in GitLab`}
        data-slot="bridge-job-link"
        onclick={(event) => openUrl(event, bridge.web_url)}>job</a
      >
    {/if}
    {#if bridge.child_url}
      <a
        href={bridge.child_url}
        class="text-muted-foreground focus-visible:ring-ring/50 shrink-0 rounded-sm text-[11px] outline-none hover:underline focus-visible:ring-2"
        title="the child pipeline"
        aria-label={`Open the child pipeline of ${bridge.name} in GitLab`}
        data-slot="bridge-child-link"
        onclick={(event) => openUrl(event, bridge.child_url)}>child</a
      >
    {:else if bridge.verdict === "dead"}
      <span class="text-tone-red shrink-0 text-[11px]" title="the trigger job created no child pipeline">no child</span>
    {/if}
  </div>
  <Collapsible.Content id={bodyId} class="pt-0.5 pb-1 pl-5">
    {#if !bridge.dived}
      <!-- `dived: false` means nobody looked, NOT that the child had no jobs. -->
      <p class="text-muted-foreground px-1 text-xs">Child pipeline not inspected.</p>
    {:else if bridge.child_id === null}
      <p class="text-muted-foreground px-1 text-xs">No child pipeline.</p>
    {:else}
      <JobList jobs={bridge.jobs} {mode} {onOpen} {now} />
    {/if}
  </Collapsible.Content>
</Collapsible.Root>
