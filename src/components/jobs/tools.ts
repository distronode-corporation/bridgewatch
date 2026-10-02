/**
 * What the job view offers on one job: the end of its log (a failed job),
 * Retry (a failed or canceled job) and Play (a GitLab manual job), and the
 * guard that makes a write go out once however fast it is clicked.
 *
 * Nothing here decides whether a write is ALLOWED. The shell's client refuses
 * a write unless the account has `actions = true`; `actions` on the watch only
 * decides whether a button is drawn.
 */

import { jobAction, jobLogTail } from "../../lib/ipc";
import { createSingleFlight } from "../../lib/single-flight";
import type { JobAction, JobActionOutcome, JobView, LogTail, Provider } from "../../lib/types";

/** What a job list needs to offer the tools, passed down from the watch. */
export interface JobTools {
  /** The watch the job is shown under: the shell resolves the job against its account. */
  watchId: string;
  /** The watch's provider. Play is GitLab's alone. */
  provider: Provider;
  /** The account allows retry and play. */
  actions: boolean;
  /** Where the job is, for the confirm text: `pipeline #42`, `child pipeline of trigger:web`. */
  context: string;
  /** The two commands, replaceable in tests. */
  api?: JobToolsApi;
}

/** The two shell commands the tools call. */
export interface JobToolsApi {
  logTail: (watch: string, url: string) => Promise<LogTail>;
  action: (watch: string, url: string, action: JobAction) => Promise<JobActionOutcome>;
}

/**
 * The shell's commands. ⚠ Wrapped rather than referenced: a test that mocks
 * `lib/ipc` without these two would otherwise fail at IMPORT of every view
 * that lists jobs, not at the one call that needs them.
 */
export const SHELL_API: JobToolsApi = {
  logTail: (watch, url) => jobLogTail(watch, url),
  action: (watch, url, action) => jobAction(watch, url, action),
};

/** A failed job has a log worth reading from the popover. */
export function canShowLog(job: JobView): boolean {
  return job.status === "failed" && !!job.web_url;
}

/** Retry: a failed or canceled job, on an account that allows it. */
export function canRetry(job: JobView, tools: JobTools): boolean {
  return tools.actions && !!job.web_url && (job.status === "failed" || job.status === "canceled");
}

/** Play: a GitLab manual job, on an account that allows it. GitHub has none. */
export function canPlay(job: JobView, tools: JobTools): boolean {
  return tools.actions && tools.provider === "gitlab" && !!job.web_url && job.status === "manual";
}

/**
 * The project a job's page is in, for the confirm text only: `group/project`
 * out of `https://host/group/project/-/jobs/1`, `owner/repo` out of a GitHub
 * job URL. Display, never routing: the shell resolves the URL itself.
 */
export function projectOf(url: string | null): string | null {
  if (!url) return null;
  let path: string;
  try {
    path = new URL(url).pathname;
  } catch {
    return null;
  }
  const gitlab = path.split("/-/jobs/")[0];
  if (gitlab !== path) return gitlab.replace(/^\/+/, "") || null;
  const github = path.match(/^\/([^/]+\/[^/]+)\/actions\/runs\//);
  return github ? github[1] : null;
}

/** The question asked before a write. Names the job, the project and where in it. */
export function confirmText(action: JobAction, job: JobView, context: string): string {
  const verb = action === "play" ? "Start manual job" : "Retry job";
  const project = projectOf(job.web_url);
  const where = project ? `${project}, ${context}` : context;
  return `${verb} "${job.name}" in ${where}?`;
}

/**
 * The one guard every job's tools share, keyed by watch, job URL and action.
 * Module-wide so a row re-rendered by a snapshot mid-request is still guarded.
 */
export const WRITES = createSingleFlight();
