<script lang="ts">
  /**
   * A pipeline's jobs: the parent's own jobs by stage, then one collapsible row
   * per bridge.
   *
   * In `"failures"` mode the parent list keeps only failures and gates, and a
   * bridge is listed only when it has one of those or a verdict that is news on
   * its own (failed, dead, passed with warnings, awaiting a gate).
   */
  import type { JobsMode, PipelineView } from "../../lib/types";
  import BridgeJobs from "./BridgeJobs.svelte";
  import JobList from "./JobList.svelte";
  import { bridgeVisible, defaultExpanded, filterJobs, type ExpansionStore } from "./jobs";

  interface Props {
    pipeline: PipelineView;
    expansion: ExpansionStore;
    mode?: JobsMode;
    onOpen: (url: string) => void;
    /** Parent stage order, when known. Else inferred from job ids. */
    stageOrder?: readonly string[] | null;
    /** See JobList: an external clock, or omit to let each list tick itself. */
    now?: number;
    /** Override the default open state of an untouched bridge. */
    isDefaultOpen?: typeof defaultExpanded;
  }

  let {
    pipeline,
    expansion,
    mode = "all",
    onOpen,
    stageOrder = null,
    now,
    isDefaultOpen = defaultExpanded,
  }: Props = $props();

  const bridges = $derived(pipeline.bridges.filter((bridge) => bridgeVisible(bridge, mode)));
  const parentShown = $derived(filterJobs(pipeline.parent_jobs, mode).length);
  const nothing = $derived(parentShown === 0 && bridges.length === 0);
</script>

<div class="flex flex-col gap-1" data-slot="pipeline-jobs" data-pipeline-id={pipeline.id} data-mode={mode}>
  {#if nothing}
    <p class="text-muted-foreground px-1 text-xs" data-slot="pipeline-jobs-empty">
      {mode === "failures" ? "No failures." : "No jobs."}
    </p>
  {:else}
    {#if parentShown > 0}
      <JobList jobs={pipeline.parent_jobs} {mode} {stageOrder} {onOpen} {now} />
    {/if}
    {#if bridges.length > 0}
      <div class="flex flex-col" role="list" aria-label="child pipelines">
        {#each bridges as bridge (bridge.name)}
          <div role="listitem">
            <BridgeJobs
              {bridge}
              pipelineId={pipeline.id}
              {expansion}
              defaultOpen={isDefaultOpen(pipeline, bridge)}
              {mode}
              {onOpen}
              {now}
            />
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>
