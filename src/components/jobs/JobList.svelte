<script lang="ts">
  /**
   * Jobs grouped by stage, stages in pipeline order.
   *
   * Every job gets a status dot in its tone, a live marker while it runs, and
   * its elapsed time, which ticks once a second between polls for a job that
   * has started. Clicking a job name calls `onOpen` with its GitLab URL; the
   * component never navigates by itself (a GitLab page inside the popover is a
   * trap), so the shell decides how a URL is opened.
   */
  import type { JobsMode, JobView } from "../../lib/types";
  import {
    TONE_DOT,
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
    class?: string;
  }

  let {
    jobs,
    mode = "all",
    stageOrder = null,
    onOpen,
    now: externalNow,
    emptyText,
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
</script>

<div class={["flex flex-col gap-1.5", className]} data-slot="job-list">
  {#if groups.length === 0}
    <p class="text-muted-foreground px-1 text-xs" data-slot="job-list-empty">{empty}</p>
  {:else}
    {#each groups as group (group.stage)}
      <section class="flex flex-col" aria-label={`stage ${group.stage}`} data-stage={group.stage}>
        <h4 class="text-muted-foreground px-1 pb-0.5 text-[11px] font-medium tracking-wide uppercase">
          {group.stage}
        </h4>
        <ul class="flex flex-col">
          {#each group.jobs as job (job.id)}
            {@const tone = toneOf(job)}
            {@const live = isLive(job)}
            {@const seconds = elapsedSeconds(job, now)}
            <li
              class="hover:bg-muted flex h-6 items-center gap-2 rounded-md px-1 text-xs"
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
                <span class="text-muted-foreground shrink-0 text-[10px]">allowed</span>
              {/if}
              <span class="flex-1"></span>
              {#if live}
                <span class="text-tone-blue shrink-0 text-[10px] font-medium uppercase" data-slot="live-marker">live</span>
              {/if}
              {#if seconds !== null}
                <span
                  class="text-muted-foreground shrink-0 tabular-nums"
                  data-slot="job-duration"
                  title={live ? "running for" : "ran for"}>{formatDuration(seconds)}</span
                >
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</div>
