<script lang="ts">
  import {
    TONE_BG,
    TONE_DOT,
    TONE_TEXT,
    deployAddsInfo,
    deployTone,
    deployWord,
    relativeAge,
    sourceBadge,
    stateTone,
    stateWord,
  } from "../lib/format";
  import { openExternal } from "../lib/ipc";
  import type { BridgeView, IconState, JobsMode, PipelineView, Provider } from "../lib/types";
  import { PipelineJobs, defaultExpanded, etaLine, type ExpansionStore } from "./jobs";
  import Link from "./Link.svelte";

  interface Props {
    row: PipelineView;
    /** One clock for the whole render, so nothing in a frame disagrees. */
    now: number;
    /** Secondary watches collapse to the top row until disclosed. */
    collapsed?: boolean;
    /** Which jobs an expanded row lists: the watch's effective `jobs` mode. */
    mode?: JobsMode;
    /** The watch's provider, which decides what a bridge is called. */
    provider?: Provider;
    /** Bridge open/closed choices, one store per window so a poll keeps them. */
    expansion: ExpansionStore;
    /** The watch this row is under, which the job tools resolve a job against. */
    watchId?: string;
    /** The watch's account allows retry and play. */
    actions?: boolean;
    /**
     * The watch's own state, as its header shows it. A row whose state is the
     * same says it to screen readers only: the verdict is shown once.
     */
    watchState?: IconState | null;
  }

  let {
    row,
    now,
    collapsed = false,
    mode = "all",
    provider = "gitlab",
    expansion,
    watchId,
    actions = false,
    watchState = null,
  }: Props = $props();

  // Only with a watch to resolve against: a row mounted without one (a test,
  // a preview) lists its jobs exactly as it always did.
  const tools = $derived(watchId ? { watchId, provider, actions, context: "" } : undefined);

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
  /** "usually ~11m · 6m 30s in", or "" when the core sent no estimate. */
  const eta = $derived(etaLine(row, now));
  /** The state word is already in the watch header right above this row. */
  const stateShown = $derived(row.state !== watchState);
  const showDeploy = $derived(deployAddsInfo(row.state, row.deploy));
  const source = $derived(sourceBadge(row.source));
</script>

<!--
  LAYOUT (L1, the two-line row). F2 and later packets: add to a line, do not
  reshape it.
    line 1 `.head`: dot · ref (13px mono, truncates) · state · deploy pill · age
    line 2 `.sub` : sha · source · "deploy job <name>" · eta   (12px muted, pl-4)
    then the jobs (`PipelineJobs`) and the sibling / after-deploy notes.
  The lines of a row are 8px apart (`gap-2`); rows in a watch are split by an
  inset hairline.
-->
<!-- `tabindex="-1"`: focusable by the popover's ↑ and ↓ (lib/keyboard.ts) when
     the row has no link to land on, never a Tab stop of its own. -->
<div
  class="row border-border/60 focus-visible:ring-ring/50 mx-3 flex flex-col gap-2 border-b py-2 outline-none last:border-b-0 focus-visible:ring-2"
  data-state={row.state}
  tabindex="-1"
>
  <!-- Line 1. The verdict is said ONCE: the state word only when the watch
       header above does not already say it (else for screen readers only),
       and the deploy pill only when it adds something the word does not. -->
  <div class="head text-ui-body flex min-w-0 items-center gap-2">
    <span class={["size-2 shrink-0 rounded-full", TONE_DOT[tone], row.live && "bw-live"]} aria-hidden="true"
    ></span>
    <span class="ref text-foreground min-w-0 flex-1 truncate font-mono" title={`ref ${row.ref}`}>{row.ref}</span>
    <span class={["state text-ui-meta shrink-0", TONE_TEXT[tone], !stateShown && "sr-only"]}>{stateWord(row.state)}</span>
    {#if showDeploy}
      <span
        class={["deploy text-ui-badge shrink-0 rounded-full px-1.5 py-px", TONE_TEXT[dTone], TONE_BG[dTone]]}
        title="deploy verdict">{deployWord(row.deploy)}</span
      >
    {/if}
    <span class="age text-muted-foreground text-ui-meta min-w-6 shrink-0 text-right tabular-nums" title={row.created_at ?? ""}
      >{relativeAge(row.created_at, now)}</span
    >
  </div>

  <!-- Line 2: what the pipeline is. `·` separators are decoration only. -->
  <div class="sub text-muted-foreground text-ui-meta flex min-w-0 flex-wrap items-baseline gap-x-1 pl-4">
    <Link href={row.web_url} class="sha text-foreground font-mono" title={row.sha}>{row.sha7}</Link>
    {#if source}
      <span aria-hidden="true">·</span>
      <span class="source" title="what started the pipeline">{source}</span>
    {/if}
    {#if !collapsed && row.deploy_marker}
      <span aria-hidden="true">·</span>
      <!-- The deploy marker, labelled: a bare job name here read as wrapped text. -->
      <span class="marker inline-flex max-w-full min-w-0 items-baseline gap-x-1">
        <span class="shrink-0">deploy job</span>
        <Link
          href={row.deploy_marker.web_url}
          class="text-foreground min-w-0 truncate font-mono"
          title={`the deploy marker job, ${row.deploy_marker.name}`}
        >
          {row.deploy_marker.name}
        </Link>
        {#if row.deploy_failures.length > 0}
          <span class="text-tone-red shrink-0">{row.deploy_failures.join(", ")}</span>
        {/if}
      </span>
    {/if}
    {#if !collapsed && eta}
      <span aria-hidden="true">·</span>
      <span class="eta tabular-nums" title="from this watch's recent deploys">{eta}</span>
    {/if}
  </div>

  {#if !collapsed}
    <!--
      Every job, parent and child, by stage (or only the failures, per the
      watch's `jobs` mode). The parent's OWN failures are in here too: a
      `secret_detection` that failed in the parent pipeline has no child to
      hang off, and a monitor that only walks bridges loses it.
    -->
    <div class="jobs pl-3">
      <PipelineJobs
        pipeline={row}
        {expansion}
        {mode}
        {provider}
        {now}
        isDefaultOpen={openByDefault}
        onOpen={(url) => void openExternal(url)}
        {tools}
      />
    </div>

    {#if row.sibling_failures.length > 0 || row.post_deploy_failures.length > 0}
      <div class="notes text-muted-foreground text-ui-meta flex flex-wrap gap-x-2 gap-y-1 pl-4">
        {#if row.sibling_failures.length > 0}
          <span class="note flex flex-wrap gap-1">
            sibling:
            {#each row.sibling_failures as name (name)}
              <Link href={jobUrl(name)} class="text-foreground font-mono">{name}</Link>
            {/each}
          </span>
        {/if}
        {#if row.post_deploy_failures.length > 0}
          <span class="note flex flex-wrap gap-1">
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
