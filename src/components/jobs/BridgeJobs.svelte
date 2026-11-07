<script lang="ts">
  /**
   * One bridge (trigger job) as a collapsible row: the verdict on the header,
   * the child pipeline's jobs by stage inside, and on the header two links out:
   * the trigger job itself and the child pipeline it created, as icons that
   * each carry an `aria-label`. They are different pages and either can be
   * the one that explains a verdict.
   *
   * A GitHub `expect` bridge (status `never_started`) has no child and no
   * trigger job: its one link is the workflow's own page, and its words say
   * the workflow never started rather than naming a child pipeline. Keyed on
   * the status the core sends, not on a provider flag, so the GitLab wording
   * is untouched.
   *
   * Any other bridge on a GitHub watch is a workflow run in a commit group. It
   * has no separate trigger job: the core sets both `web_url` and `child_url`
   * to the run's page, so it gets ONE link (an icon named "Open workflow run
   * <name>"), and words that do not name GitLab. The provider comes from the watch (`WatchView.provider`),
   * because nothing on the bridge itself says which kind it is.
   *
   * Open/closed lives in the `expansion` store, keyed by pipeline id + bridge
   * name, so a snapshot refresh that replaces `bridge` with a new object does
   * not collapse it.
   */
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import ExternalLinkIcon from "@lucide/svelte/icons/external-link";
  import WorkflowIcon from "@lucide/svelte/icons/workflow";

  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import { TONE_DOT, TONE_TEXT, bridgeShortWord, bridgeTone, bridgeWord } from "../../lib/format";
  import type { BridgeView, JobsMode, Provider } from "../../lib/types";
  import JobList from "./JobList.svelte";
  import { filterJobs, type ExpansionStore } from "./jobs";
  import type { JobTools } from "./tools";

  interface Props {
    bridge: BridgeView;
    pipelineId: number;
    expansion: ExpansionStore;
    /** Open state for a bridge the user never toggled. */
    defaultOpen?: boolean;
    mode?: JobsMode;
    /** The watch's provider. Absent means GitLab. */
    provider?: Provider;
    onOpen: (url: string) => void;
    /** See JobList: an external clock, or omit to let the list tick itself. */
    now?: number;
    /** The log, retry and play tools; the bridge fills in its own context. */
    tools?: JobTools;
  }

  let {
    bridge,
    pipelineId,
    expansion,
    defaultOpen = false,
    mode = "all",
    provider = "gitlab",
    onOpen,
    now,
    tools,
  }: Props = $props();

  const open = $derived(expansion.isOpen(pipelineId, bridge.name, defaultOpen));
  const tone = $derived(bridgeTone(bridge.verdict));
  const shownCount = $derived(filterJobs(bridge.jobs, mode).length);
  const neverStarted = $derived(bridge.status === "never_started");
  const workflowRun = $derived(provider === "github" && !neverStarted);
  // Where these jobs are, for the confirm text.
  const childTools = $derived(
    tools && {
      ...tools,
      context: workflowRun
        ? `workflow run ${bridge.name}`
        : `child pipeline ${bridge.child_id === null ? "" : `#${bridge.child_id} `}of ${bridge.name}`,
    },
  );
  const bodyId = $derived(`bridge-${pipelineId}-${bridge.name.replace(/[^A-Za-z0-9_-]/g, "_")}`);

  const LINK =
    "text-muted-foreground hover:text-foreground hover:bg-background focus-visible:ring-ring/50 flex size-6 shrink-0 items-center justify-center rounded-md outline-none focus-visible:ring-2";

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
  <!-- Columns: the name truncates (`flex-1`), the status sits right, the
       count is a fixed `w-6` column, and the links are icons in a fixed
       column, so every bridge in a pipeline lines up with the next. -->
  <div class="hover:bg-muted text-ui-meta flex h-7 items-center gap-1 rounded-lg pr-1">
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
      <!-- The NAME has priority: it keeps at least 12ch, and the status gives
           way first (it truncates, and in a narrow window a long verdict is
           shortened; see .bw-wide-only in app.css). The trigger's aria-label
           and the status's title keep the full verdict. -->
      <span class="name text-foreground min-w-[12ch] flex-1 truncate font-mono" title={bridge.name}>{bridge.name}</span>
      <span class={["status ml-auto min-w-0 truncate", TONE_TEXT[tone]]} title={bridgeWord(bridge.verdict)}>
        {#if bridgeShortWord(bridge.verdict)}
          <span class="bw-wide-only">{bridgeWord(bridge.verdict)}</span><span class="bw-narrow-only"
            >{bridgeShortWord(bridge.verdict)}</span
          >
        {:else}
          {bridgeWord(bridge.verdict)}
        {/if}
      </span>
      <span class="count text-muted-foreground w-6 shrink-0 text-right tabular-nums" title="jobs listed / jobs in the child"
        >{#if bridge.dived && bridge.jobs.length > 0}{mode === "all"
            ? bridge.jobs.length
            : `${shownCount}/${bridge.jobs.length}`}{/if}</span
      >
    </Collapsible.Trigger>
    <!-- ⛔ Both links are SIBLINGS of the trigger, never inside it: the bridge
         name sits in a button, and an anchor nested in a button is neither
         valid nor reachable by keyboard. A bridge with no `web_url` gets no
         link rather than a dead one; its name is already plain text above.
         ⛔ Icon-only, so the `aria-label` is the link's whole name. -->
    <span class="links flex min-w-12 shrink-0 items-center justify-end gap-0.5">
      {#if bridge.verdict === "dead" && !bridge.child_url && !workflowRun}
        {#if neverStarted}
          <span class="text-tone-red shrink-0 pr-1" title="expected workflow did not run" data-slot="bridge-never-started"
            >never started</span
          >
        {:else}
          <span class="text-tone-red shrink-0 pr-1" title="the trigger job created no child pipeline">no child</span>
        {/if}
      {/if}
      {#if workflowRun}
        {#if bridge.web_url ?? bridge.child_url}
          <a
            href={bridge.web_url ?? bridge.child_url}
            class={LINK}
            title="the workflow run"
            aria-label={`Open workflow run ${bridge.name}`}
            data-slot="bridge-run-link"
            onclick={(event) => openUrl(event, bridge.web_url ?? bridge.child_url)}
            ><ExternalLinkIcon class="size-3.5" aria-hidden="true" /></a
          >
        {/if}
      {:else}
        {#if bridge.child_url}
          <a
            href={bridge.child_url}
            class={LINK}
            title="the child pipeline"
            aria-label={`Open the child pipeline of ${bridge.name} in GitLab`}
            data-slot="bridge-child-link"
            onclick={(event) => openUrl(event, bridge.child_url)}
            ><WorkflowIcon class="size-3.5" aria-hidden="true" /></a
          >
        {/if}
        {#if bridge.web_url}
          <a
            href={bridge.web_url}
            class={LINK}
            title={neverStarted ? "the expected workflow" : "the trigger job"}
            aria-label={neverStarted ? `Open workflow ${bridge.name}` : `Open trigger job ${bridge.name} in GitLab`}
            data-slot="bridge-job-link"
            onclick={(event) => openUrl(event, bridge.web_url)}
            ><ExternalLinkIcon class="size-3.5" aria-hidden="true" /></a
          >
        {/if}
      {/if}
    </span>
  </div>
  <Collapsible.Content id={bodyId} class="text-ui-meta pt-1 pb-1 pl-5">
    {#if neverStarted}
      <p class="text-muted-foreground px-1">Expected workflow did not run.</p>
    {:else if workflowRun && !bridge.dived}
      <p class="text-muted-foreground px-1">Workflow run not inspected.</p>
    {:else if !bridge.dived}
      <!-- `dived: false` means nobody looked, NOT that the child had no jobs. -->
      <p class="text-muted-foreground px-1">Child pipeline not inspected.</p>
    {:else if bridge.child_id === null}
      <p class="text-muted-foreground px-1">No child pipeline.</p>
    {:else}
      <JobList jobs={bridge.jobs} {mode} {onOpen} {now} tools={childTools} />
    {/if}
  </Collapsible.Content>
</Collapsible.Root>
