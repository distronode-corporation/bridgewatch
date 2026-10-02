/**
 * The job view's logic, kept out of the components so it can be tested as
 * plain functions.
 *
 * Nothing here judges a job: the class (`passed`, `blocking_failure`, ...) and
 * the effective jobs mode come from the core. This module only groups, filters
 * and formats what the core already decided.
 */

import { SvelteMap } from "svelte/reactivity";

import { jobTone, type Tone } from "../../lib/format";
import type { BridgeView, JobClass, JobsMode, JobView, PipelineView } from "../../lib/types";

export type { JobsMode };

/** The label used for jobs the API returned without a stage. */
export const NO_STAGE = "(no stage)";

export interface StageGroup {
  stage: string;
  jobs: JobView[];
}

/**
 * The job classes a `"failures"` view keeps: blocking and tolerated failures,
 * plus manual gates, which are the other thing a person has to act on. The same
 * set the popover listed before the full job view existed.
 */
export const FAILURE_VIEW_CLASSES: ReadonlySet<JobClass> = new Set<JobClass>([
  "blocking_failure",
  "warning_failure",
  "gate",
]);

/** Bridge verdicts that are news even when no job in the child says so. */
const NOTABLE_BRIDGE_VERDICTS = new Set(["failed", "dead", "passed_with_warnings", "awaiting_gate"]);

/** The jobs a mode shows, in their original order. */
export function filterJobs(jobs: readonly JobView[], mode: JobsMode): JobView[] {
  return mode === "all" ? [...jobs] : jobs.filter((job) => FAILURE_VIEW_CLASSES.has(job.class));
}

/** Whether a bridge has anything to show in a mode. `"all"` shows every bridge. */
export function bridgeVisible(bridge: BridgeView, mode: JobsMode): boolean {
  if (mode === "all") return true;
  return NOTABLE_BRIDGE_VERDICTS.has(bridge.verdict) || filterJobs(bridge.jobs, mode).length > 0;
}

/**
 * Group jobs by stage, stages in pipeline order.
 *
 * GitLab's jobs API does not say where a stage sits in the pipeline. When the
 * caller knows (`stageOrder`), that order wins. Otherwise stages are ordered by
 * the LOWEST job id in each: GitLab creates a pipeline's jobs stage by stage,
 * so creation order is stage order, and the lowest id survives a retry (a
 * retried job gets a new, higher id, but the stage's other jobs keep theirs).
 * Stages missing from `stageOrder` follow the listed ones, by the same rule.
 * Jobs with no stage go last. Within a stage, jobs keep the order they came in.
 */
export function groupByStage(jobs: readonly JobView[], stageOrder?: readonly string[] | null): StageGroup[] {
  const groups = new Map<string, { jobs: JobView[]; minId: number }>();
  for (const job of jobs) {
    const stage = job.stage && job.stage.length > 0 ? job.stage : NO_STAGE;
    const group = groups.get(stage);
    if (group) {
      group.jobs.push(job);
      group.minId = Math.min(group.minId, job.id);
    } else {
      groups.set(stage, { jobs: [job], minId: job.id });
    }
  }

  const explicit = new Map<string, number>();
  stageOrder?.forEach((stage, index) => {
    if (!explicit.has(stage)) explicit.set(stage, index);
  });

  return [...groups.entries()]
    .sort(([a, ga], [b, gb]) => {
      if (a === NO_STAGE || b === NO_STAGE) return a === NO_STAGE ? (b === NO_STAGE ? 0 : 1) : -1;
      const ia = explicit.get(a);
      const ib = explicit.get(b);
      if (ia !== undefined && ib !== undefined) return ia - ib;
      if (ia !== undefined) return -1;
      if (ib !== undefined) return 1;
      return ga.minId - gb.minId;
    })
    .map(([stage, group]) => ({ stage, jobs: group.jobs }));
}

/**
 * Whether a job gets the live marker: the core's `live` class, which covers
 * pending as well as running jobs.
 */
export function isLive(job: JobView): boolean {
  return job.class === "live";
}

/** Whether a job's time ticks locally: live AND actually started. */
export function isTicking(job: JobView): boolean {
  return isLive(job) && !!job.started_at && !Number.isNaN(Date.parse(job.started_at));
}

/**
 * Seconds to show for a job at `now` (ms since the epoch), or null for none.
 *
 * A running job ticks from `started_at`, clamped at zero so a clock a few
 * seconds ahead of GitLab's does not print a negative time. A finished job
 * shows GitLab's `duration`, else `finished_at - started_at`. A job that never
 * started shows nothing.
 */
export function elapsedSeconds(job: JobView, now: number): number | null {
  const started = job.started_at ? Date.parse(job.started_at) : Number.NaN;
  if (isLive(job)) {
    if (!Number.isNaN(started)) return Math.max(0, Math.floor((now - started) / 1000));
    // Live but no start time (pending), or a recording without one: GitLab's
    // elapsed time at the last poll, if it sent one, without ticking.
    return typeof job.duration === "number" ? Math.max(0, Math.floor(job.duration)) : null;
  }
  if (typeof job.duration === "number") return Math.max(0, Math.round(job.duration));
  const finished = job.finished_at ? Date.parse(job.finished_at) : Number.NaN;
  if (!Number.isNaN(started) && !Number.isNaN(finished)) {
    return Math.max(0, Math.round((finished - started) / 1000));
  }
  return null;
}

/** "42s", "3m 07s", "1h 02m". */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "";
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s}s`;
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return `${minutes}m ${String(s % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/**
 * "~45s", "~11m", "~1h 05m": a typical duration, to the nearest minute once it
 * is one. The CLI's `approx_duration` says the same.
 */
export function approxDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "";
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `~${s}s`;
  // Rounded before the unit is chosen, so 59m 40s reads "~1h 00m", never "~60m".
  const minutes = Math.floor((s + 30) / 60);
  if (minutes < 60) return `~${minutes}m`;
  return `~${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/**
 * "usually ~11m · 6m 30s in", for a row the core gave an estimate, else "".
 *
 * Whether there IS an estimate is the core's call (enough samples, still
 * running, not past twice the usual time), and this never second-guesses it.
 * Only the elapsed half ticks locally from `created_at`, as a running job's
 * time does, so it does not stand still between polls; without a readable
 * `created_at` it shows the core's figure as it was at the last poll.
 */
export function etaLine(row: Pick<PipelineView, "eta" | "created_at">, now: number): string {
  const eta = row.eta;
  if (!eta) return "";
  const created = row.created_at ? Date.parse(row.created_at) : Number.NaN;
  const elapsed = Number.isNaN(created) ? eta.elapsed_secs : Math.max(0, Math.floor((now - created) / 1000));
  return `usually ${approxDuration(eta.typical_secs)} · ${formatDuration(elapsed)} in`;
}

/** "updated 4s ago". `now` and the result of `Date.parse(at)` are ms. */
export function updatedAgo(at: string | number | null | undefined, now: number): string {
  if (at === null || at === undefined || at === "") return "not updated yet";
  const then = typeof at === "number" ? at : Date.parse(at);
  if (Number.isNaN(then)) return "not updated yet";
  const seconds = Math.max(0, Math.floor((now - then) / 1000));
  if (seconds < 60) return `updated ${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `updated ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `updated ${hours}h ago`;
  return `updated ${Math.floor(hours / 24)}d ago`;
}

/** A job's tone, from its core-decided class. */
export function toneOf(job: JobView): Tone {
  return jobTone(job.class);
}

/** Words for a job class, for screen readers and tooltips. */
export function classWord(jobClass: JobClass): string {
  switch (jobClass) {
    case "passed":
      return "passed";
    case "live":
      return "running";
    case "blocking_failure":
      return "failed";
    case "warning_failure":
      return "failed, allowed to fail";
    case "gate":
      return "waiting at a manual gate";
    case "not_built":
      return "not built";
    case "ignored":
      return "ignored";
    default:
      return "unknown";
  }
}

// ---------------------------------------------------------------------------
// Expansion state
// ---------------------------------------------------------------------------

/** The key a bridge's open/closed state is stored under. */
export function expansionKey(pipelineId: number, bridgeName: string): string {
  return `${pipelineId}\u0000${bridgeName}`;
}

/**
 * The default for a bridge nobody has toggled: open while its pipeline is
 * live, closed once it settled.
 */
export function defaultExpanded(pipeline: Pick<PipelineView, "live">, _bridge?: BridgeView): boolean {
  return pipeline.live;
}

export interface ExpansionStore {
  /** Open or closed: the user's choice when there is one, else `fallback`. */
  isOpen(pipelineId: number, bridgeName: string, fallback: boolean): boolean;
  set(pipelineId: number, bridgeName: string, open: boolean): void;
  toggle(pipelineId: number, bridgeName: string, fallback: boolean): boolean;
  /** Whether the user has made a choice for this bridge. */
  has(pipelineId: number, bridgeName: string): boolean;
  /** Forget choices for pipelines no longer on screen. */
  prune(keepPipelineIds: Iterable<number>): void;
  readonly size: number;
}

/**
 * Where bridge open/closed choices live.
 *
 * Keyed by pipeline id + bridge name, NOT by position and NOT inside the row
 * components, so a snapshot refresh (which replaces every row object and may
 * reorder them) never collapses what the user opened. Only explicit choices are
 * stored; an untouched bridge follows `defaultExpanded` as its pipeline goes
 * from live to settled. Create one per window and pass it down.
 */
export function createExpansionStore(): ExpansionStore {
  const choices = new SvelteMap<string, boolean>();
  return {
    isOpen(pipelineId, bridgeName, fallback) {
      return choices.get(expansionKey(pipelineId, bridgeName)) ?? fallback;
    },
    set(pipelineId, bridgeName, open) {
      choices.set(expansionKey(pipelineId, bridgeName), open);
    },
    toggle(pipelineId, bridgeName, fallback) {
      const next = !this.isOpen(pipelineId, bridgeName, fallback);
      this.set(pipelineId, bridgeName, next);
      return next;
    },
    has(pipelineId, bridgeName) {
      return choices.has(expansionKey(pipelineId, bridgeName));
    },
    prune(keepPipelineIds) {
      const keep = new Set([...keepPipelineIds].map((id) => `${id}\u0000`));
      for (const key of [...choices.keys()]) {
        const prefix = key.slice(0, key.indexOf("\u0000") + 1);
        if (!keep.has(prefix)) choices.delete(key);
      }
    },
    get size() {
      return choices.size;
    },
  };
}
