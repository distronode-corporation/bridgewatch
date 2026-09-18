<script lang="ts">
  import { Badge } from "$lib/components/ui/badge/index.js";
  import {
    TONE_BG,
    TONE_DOT,
    TONE_TEXT,
    deployTone,
    deployWord,
    relativeAge,
    sourceBadge,
    stateTone,
    stateWord,
  } from "../lib/format";
  import { openExternal } from "../lib/ipc";
  import type { BridgeView, JobsMode, PipelineView } from "../lib/types";
  import { PipelineJobs, defaultExpanded, type ExpansionStore } from "./jobs";
  import Link from "./Link.svelte";

  interface Props {
    row: PipelineView;
    /** One clock for the whole render, so nothing in a frame disagrees. */
    now: number;
    /** Secondary watches collapse to the top row until disclosed. */
    collapsed?: boolean;
    /** Which jobs an expanded row lists: the watch's effective `jobs` mode. */
    mode?: JobsMode;
    /** Bridge open/closed choices, one store per window so a poll keeps them. */
    expansion: ExpansionStore;
  }

  let { row, now, collapsed = false, mode = "all", expansion }: Props = $props();

  /**
   * A bridge nobody has toggled: open while its pipeline is live (the job
   * view's own rule), and ALSO open when the bridge failed or never produced a
   * child. 🔑 The claim the program exists to make is "the website deployed AND
   * android failed, with the failing job one click away"; a settled pipeline
   * collapsing that bridge would put it two clicks away.
   */
  function openByDefault(pipeline: Pick<PipelineView, "live">, bridge?: BridgeView): boolean {
    if (defaultExpanded(pipeline, bridge)) return true;
    return bridge?.verdict === "failed" || bridge?.verdict === "dead";
  }

  /**
   * The URL for a name the core reported as a sibling or post-deploy failure.
   *
   * ⛔ `parent_jobs` alone is not enough, and it is the common case that it
   * misses: a sibling failure is usually a BRIDGE (`trigger:android`), and
   * GitLab's jobs endpoint does not return bridges, so every one of those
   * names rendered as plain text next to a row full of working links. The
   * bridge's own `web_url` is the trigger job, which is exactly the page a
   * person clicking that name wants; a child job is looked up last, for a
   * post-deploy failure that happened inside a child pipeline.
   */
  function jobUrl(name: string): string | null {
    const parent = row.parent_jobs.find((job) => job.name === name);
    if (parent?.web_url) return parent.web_url;
    const bridge = row.bridges.find((b) => b.name === name);
    if (bridge?.web_url) return bridge.web_url;
    for (const b of row.bridges) {
      const job = b.jobs.find((j) => j.name === name);
      if (job?.web_url) return job.web_url;
    }
    return null;
  }

  const tone = $derived(stateTone(row.state));
  const dTone = $derived(deployTone(row.deploy));
</script>

<div class="row border-border/60 flex flex-col gap-1 border-b px-3 pt-1.5 pb-2 last:border-b-0">
  <div class="head flex items-center gap-1.5 text-[13px]">
    <span class={["size-2 shrink-0 rounded-full", TONE_DOT[tone], row.live && "bw-live"]} aria-hidden="true"
    ></span>
    <Link href={row.web_url} class="sha text-foreground font-mono text-xs font-semibold" title={row.sha}
      >{row.sha7}</Link
    >
    <span class="ref text-muted-foreground max-w-36 truncate font-mono text-xs" title="ref">{row.ref}</span>
    {#if row.source}
      <Badge variant="secondary" class="source h-4 px-1.5 text-[10px]">{sourceBadge(row.source)}</Badge>
    {/if}
    <span class="flex-1"></span>
    <span class={["state text-xs", TONE_TEXT[tone]]}>{stateWord(row.state)}</span>
    <span
      class={["deploy rounded-full px-1.5 text-[11px] leading-[17px]", TONE_TEXT[dTone], TONE_BG[dTone]]}
      title="deploy verdict">{deployWord(row.deploy)}</span
    >
    <span class="age text-muted-foreground min-w-6 text-right text-[11px] tabular-nums" title={row.created_at ?? ""}
      >{relativeAge(row.created_at, now)}</span
    >
  </div>

  {#if !collapsed}
    {#if row.deploy_marker}
      <div class="marker flex flex-wrap gap-1.5 pl-3.5 text-xs">
        <Link href={row.deploy_marker.web_url} class="font-mono" title="the deploy marker job">
          {row.deploy_marker.name}
        </Link>
        {#if row.deploy_failures.length > 0}
          <span class="text-tone-red">{row.deploy_failures.join(", ")}</span>
        {/if}
      </div>
    {/if}

    <!--
      Every job, parent and child, by stage (or only the failures, per the
      watch's `jobs` mode). The parent's OWN failures are in here too: a
      `secret_detection` that failed in the parent pipeline has no child to
      hang off, and a monitor that only walks bridges loses it.
    -->
    <div class="pl-2">
      <PipelineJobs
        pipeline={row}
        {expansion}
        {mode}
        {now}
        isDefaultOpen={openByDefault}
        onOpen={(url) => void openExternal(url)}
      />
    </div>

    {#if row.sibling_failures.length > 0 || row.post_deploy_failures.length > 0}
      <div class="notes text-muted-foreground flex flex-wrap gap-1.5 pl-3.5 text-xs">
        {#if row.sibling_failures.length > 0}
          <span class="note flex flex-wrap gap-1.5">
            sibling:
            {#each row.sibling_failures as name (name)}
              <Link href={jobUrl(name)} class="text-foreground font-mono">{name}</Link>
            {/each}
          </span>
        {/if}
        {#if row.post_deploy_failures.length > 0}
          <span class="note flex flex-wrap gap-1.5">
            after deploy:
            {#each row.post_deploy_failures as name (name)}
              <Link href={jobUrl(name)} class="text-foreground font-mono">{name}</Link>
            {/each}
          </span>
        {/if}
      </div>
    {/if}
  {/if}
</div>
