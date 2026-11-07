<script lang="ts">
  /**
   * Jobs grouped by stage, stages in pipeline order.
   *
   * Every job gets a status dot in its tone, a live marker while it runs, and
   * its elapsed time, which ticks once a second between polls for a job that
   * has started. Clicking a job name calls `onOpen` with its GitLab URL; the
   * component never navigates by itself (a GitLab page inside the popover is a
   * trap), so the shell decides how a URL is opened.
   *
   * With `tools`, a failed job also gets a "log" toggle that opens the end of
   * its log under the row, and on an account with `actions = true` a failed or
   * canceled job gets "retry" and a GitLab manual job "play", each of which
   * asks first (see JobPanel). Without `tools` the list is exactly what it was.
   *
   * A failed job whose provider said why has its first failing line under its
   * row (12px mono, one line, the whole of it on hover), with or without
   * `tools`. With them a failed job also gets "copy", which puts the failure on
   * the clipboard as Markdown (`failureMarkdown`): at the end of the failing
   * line when there is one, so the line truncates rather than the job's name
   * (at 340px a third word in the row's tools cut `secret_detection` short),
   * and among the row's tools when there is not.
   */
  import { TONE_DOT } from "../../lib/format";
  import type { JobAction, JobsMode, JobView } from "../../lib/types";
  import JobPanel from "./JobPanel.svelte";
  import {
    SHELL_API,
    canCopyFailure,
    canPlay,
    canRetry,
    canShowLog,
    failureMarkdown,
    type JobTools,
  } from "./tools";
  import {
    classWord,
    elapsedSeconds,
    filterJobs,
    formatDuration,
    groupByStage,
    isLive,
    isTicking,
    toneOf,
  } from "./jobs";

  interface Props {
    jobs: readonly JobView[];
    /** `"all"` lists every job; `"failures"` only failures and manual gates. */
    mode?: JobsMode;
    /** The pipeline's stage order, when the caller knows it. Else inferred from job ids. */
    stageOrder?: readonly string[] | null;
    /** Open a GitLab URL. Called instead of navigating. */
    onOpen: (url: string) => void;
    /**
     * An external clock in ms. When given, the list renders against it and runs
     * no timer of its own (one clock for a whole popover). When omitted, the
     * list ticks itself once a second while any listed job is running.
     */
    now?: number;
    /** Shown when the mode filters every job out. */
    emptyText?: string;
    /** The log, retry and play tools, or none. */
    tools?: JobTools;
    class?: string;
  }

  let {
    jobs,
    mode = "all",
    stageOrder = null,
    onOpen,
    now: externalNow,
    emptyText,
    tools,
    class: className = "",
  }: Props = $props();

  const empty = $derived(emptyText ?? (mode === "failures" ? "No failures." : "No jobs."));
  const shown = $derived(filterJobs(jobs, mode));
  const groups = $derived(groupByStage(shown, stageOrder));
  const ticking = $derived(externalNow === undefined && shown.some(isTicking));

  let ownNow = $state(Date.now());
  const now = $derived(externalNow ?? ownNow);

  $effect(() => {
    if (!ticking) return;
    ownNow = Date.now();
    const timer = setInterval(() => {
      ownNow = Date.now();
    }, 1000);
    return () => clearInterval(timer);
  });

  function open(event: MouseEvent, url: string) {
    event.preventDefault();
    event.stopPropagation();
    onOpen(url);
  }

  // What is open under each job, by job id. Kept here rather than in the
  // panel so a row's buttons can say `aria-expanded` truthfully.
  let panels = $state<Record<number, { log: boolean; confirm: JobAction | null }>>({});
  const panelOf = (id: number) => panels[id] ?? { log: false, confirm: null };
  const panelId = (id: number) => `job-panel-${(tools?.watchId ?? "").replace(/[^A-Za-z0-9_-]/g, "_")}-${id}`;

  function toggleLog(id: number) {
    const current = panelOf(id);
    panels[id] = { ...current, log: !current.log };
  }

  function ask(id: number, action: JobAction) {
    const current = panelOf(id);
    panels[id] = { ...current, confirm: current.confirm === action ? null : action };
  }

  function cancel(id: number) {
    panels[id] = { ...panelOf(id), confirm: null };
  }

  // "copied" or "not copied" after a copy, by job id, for a moment.
  let copied = $state<Record<number, "copied" | "failed">>({});
  const copyTimers = new Map<number, ReturnType<typeof setTimeout>>();
  $effect(() => () => copyTimers.forEach((timer) => clearTimeout(timer)));

  async function copyFailure(job: JobView) {
    if (!tools) return;
    const copy = tools.api?.copy ?? SHELL_API.copy;
    const ok = copy ? await copy(failureMarkdown(job, tools.context)) : false;
    copied[job.id] = ok ? "copied" : "failed";
    clearTimeout(copyTimers.get(job.id));
    copyTimers.set(
      job.id,
      setTimeout(() => {
        delete copied[job.id];
        copyTimers.delete(job.id);
      }, 1500),
    );
  }

  // 12px words, not 10px capitals.
  const TOOL_BUTTON =
    "text-muted-foreground hover:text-foreground hover:bg-background focus-visible:ring-ring/50 aria-expanded:text-foreground shrink-0 rounded-sm px-1 outline-none focus-visible:ring-2";

  /**
   * The tools of a failed job are always shown. Any other job's (retry on a
   * canceled one, play on a manual one) appear on hover or keyboard focus.
   *
   * ⛔ Hidden with opacity, never `invisible` or `display: none`: those take
   * the buttons out of the tab order, and the focus that should reveal them
   * could then never reach them. An open panel keeps its row's tools shown
   * (from state, not `:has()`, which WebKitGTK 2.36 lacks; see app.css).
   */
  const toolsShown = (job: JobView) =>
    job.status === "failed" || panelOf(job.id).log || panelOf(job.id).confirm !== null;

  /** The first failing line is drawn under this job. */
  const hasLine = (job: JobView) => !!job.failure && job.status === "failed";
  /** "copy" goes in the row's tools: a failed job with no line to put it on. */
  const copyInRow = (job: JobView) => canCopyFailure(job) && !hasLine(job);
</script>

{#snippet copyButton(job: JobView)}
  <button
    type="button"
    class={TOOL_BUTTON}
    aria-label={`Copy the failure of ${job.name} as Markdown`}
    title="Copy failure as Markdown"
    data-slot="job-copy"
    data-copied={copied[job.id]}
    onclick={() => void copyFailure(job)}
    >{copied[job.id] === "copied" ? "copied" : copied[job.id] === "failed" ? "not copied" : "copy"}</button
  >
  <!-- The label above names the action; this says how it went. -->
  <span class="sr-only" role="status" data-slot="job-copy-status"
    >{copied[job.id] === "copied"
      ? `Copied the failure of ${job.name}`
      : copied[job.id] === "failed"
        ? "Could not copy to the clipboard"
        : ""}</span
  >
{/snippet}

<div class={["text-ui-meta flex flex-col gap-2", className]} data-slot="job-list">
  {#if groups.length === 0}
    <p class="text-muted-foreground px-1" data-slot="job-list-empty">{empty}</p>
  {:else}
    {#each groups as group (group.stage)}
      <section class="flex flex-col" aria-label={`stage ${group.stage}`} data-stage={group.stage}>
        <h4 class="text-muted-foreground text-ui-badge px-1 pb-0.5 font-medium tracking-wide uppercase">
          {group.stage}
        </h4>
        <ul class="flex flex-col">
          {#each group.jobs as job (job.id)}
            {@const tone = toneOf(job)}
            {@const live = isLive(job)}
            {@const seconds = elapsedSeconds(job, now)}
            <!-- Columns: name (truncates) · tools · live · duration (right). -->
            <li
              class="group/job hover:bg-muted flex h-6 items-center gap-2 rounded-md px-1"
              data-job-id={job.id}
              data-class={job.class}
            >
              <span
                class={["size-2 shrink-0 rounded-full", TONE_DOT[tone], live && "bw-live"]}
                data-slot="job-dot"
                aria-hidden="true"
              ></span>
              <span class="sr-only">{classWord(job.class)}:</span>
              {#if job.web_url}
                <a
                  href={job.web_url}
                  class="text-foreground focus-visible:ring-ring/50 min-w-0 truncate rounded-sm font-mono outline-none hover:underline focus-visible:ring-2"
                  title={`${job.name} (${classWord(job.class)})`}
                  onclick={(event) => open(event, job.web_url as string)}>{job.name}</a
                >
              {:else}
                <span class="text-foreground min-w-0 truncate font-mono" title={job.name}>{job.name}</span>
              {/if}
              {#if job.allow_failure && job.class === "warning_failure"}
                <span class="text-muted-foreground text-ui-badge shrink-0">allowed</span>
              {/if}
              <span class="flex-1"></span>
              {#if tools && (canShowLog(job) || copyInRow(job) || canRetry(job, tools) || canPlay(job, tools))}
                <span
                  class={[
                    "flex shrink-0 items-center gap-0.5",
                    !toolsShown(job) && "opacity-0 group-hover/job:opacity-100 focus-within:opacity-100",
                  ]}
                  data-slot="job-tools"
                  data-always={toolsShown(job) ? "" : undefined}
                >
                  {#if canShowLog(job)}
                    <button
                      type="button"
                      class={TOOL_BUTTON}
                      aria-expanded={panelOf(job.id).log}
                      aria-controls={panelId(job.id)}
                      aria-label={`${panelOf(job.id).log ? "Hide" : "Show"} the end of the log of ${job.name}`}
                      data-slot="job-log-toggle"
                      onclick={() => toggleLog(job.id)}>log</button
                    >
                  {/if}
                  {#if copyInRow(job)}
                    {@render copyButton(job)}
                  {/if}
                  {#if canRetry(job, tools)}
                    <button
                      type="button"
                      class={TOOL_BUTTON}
                      aria-expanded={panelOf(job.id).confirm === "retry"}
                      aria-controls={panelId(job.id)}
                      aria-label={`Retry ${job.name}`}
                      data-slot="job-retry"
                      onclick={() => ask(job.id, "retry")}>retry</button
                    >
                  {/if}
                  {#if canPlay(job, tools)}
                    <button
                      type="button"
                      class={TOOL_BUTTON}
                      aria-expanded={panelOf(job.id).confirm === "play"}
                      aria-controls={panelId(job.id)}
                      aria-label={`Start ${job.name}`}
                      data-slot="job-play"
                      onclick={() => ask(job.id, "play")}>play</button
                    >
                  {/if}
                </span>
              {/if}
              {#if live}
                <span class="text-tone-blue text-ui-badge shrink-0 font-medium uppercase" data-slot="live-marker">live</span>
              {/if}
              {#if seconds !== null}
                <span
                  class="text-muted-foreground min-w-10 shrink-0 text-right tabular-nums"
                  data-slot="job-duration"
                  title={live ? "running for" : "ran for"}>{formatDuration(seconds)}</span
                >
              {/if}
            </li>
            {#if hasLine(job)}
              <!-- The first failing line, under its job: aligned with the name
                   (dot 8px + gap 8px + the row's 4px), one line, whole on
                   hover; "copy" at its end, in the duration's column. -->
              <li
                class="text-muted-foreground text-ui-meta flex min-w-0 items-center gap-2 pr-1 pb-0.5 pl-5"
                data-slot="job-failure"
                data-job-id={job.id}
              >
                <span class="min-w-0 flex-1 truncate font-mono" title={job.failure} data-slot="job-failure-text">
                  <span class="sr-only">first failing line of {job.name}:</span>
                  {job.failure}
                </span>
                {#if tools}
                  <span class="flex shrink-0 items-center">{@render copyButton(job)}</span>
                {/if}
              </li>
            {/if}
            {#if tools && (panelOf(job.id).log || panelOf(job.id).confirm)}
              <li>
                <JobPanel
                  {job}
                  {tools}
                  log={panelOf(job.id).log}
                  confirm={panelOf(job.id).confirm}
                  onCancel={() => cancel(job.id)}
                  {onOpen}
                  id={panelId(job.id)}
                />
              </li>
            {/if}
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</div>
