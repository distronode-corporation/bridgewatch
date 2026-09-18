/**
 * The only logic in the frontend: turning a value the core already decided into
 * something readable.
 *
 * Nothing here may make a judgement. `verdictTone` maps a state name the core
 * produced onto a colour class; it does not decide what the state IS. If a
 * function in this file ever needs to know what a failure MEANS, it belongs in
 * `bridgewatch-core`.
 */

import type { DiagnosticView, IconState, Status } from "./types";

/** A colour family, resolved to real colours by CSS custom properties. */
export type Tone = "red" | "amber" | "green" | "blue" | "grey";

const STATE_TONES: Record<IconState, Tone> = {
  unknown: "grey",
  failed: "red",
  deployed_with_failure: "amber",
  deployed: "green",
  running: "blue",
  canceled: "grey",
  parked_gate: "amber",
  succeeded_no_deploy: "green",
};

const STATE_WORDS: Record<IconState, string> = {
  unknown: "unknown",
  failed: "failed",
  deployed_with_failure: "deployed with failure",
  deployed: "deployed",
  running: "running",
  canceled: "canceled",
  parked_gate: "parked at a gate",
  succeeded_no_deploy: "green, no deploy",
};

/** The tone for a pipeline or tray state. */
export function stateTone(state: IconState | null | undefined): Tone {
  if (!state) return "grey";
  return STATE_TONES[state] ?? "grey";
}

/** The state as a phrase, not a snake_case identifier. */
export function stateWord(state: IconState | null | undefined): string {
  if (!state) return "unknown";
  return STATE_WORDS[state] ?? state;
}

const BRIDGE_TONES: Record<string, Tone> = {
  passed: "green",
  passed_with_warnings: "amber",
  failed: "red",
  // A bridge whose child pipeline was never created. Red, because a trigger
  // that produced nothing is a failure that every other monitor reports green.
  dead: "red",
  running: "blue",
  awaiting_gate: "amber",
  canceled: "grey",
  skipped: "grey",
  unknown: "grey",
};

/** The tone for a `BridgeView.verdict`. */
export function bridgeTone(verdict: string): Tone {
  return BRIDGE_TONES[verdict] ?? "grey";
}

/** The bridge verdict as a phrase. */
export function bridgeWord(verdict: string): string {
  return verdict === "passed_with_warnings"
    ? "passed, with warnings"
    : verdict === "awaiting_gate"
      ? "awaiting a gate"
      : verdict;
}

const DEPLOY_TONES: Record<string, Tone> = {
  absent: "grey",
  in_progress: "blue",
  live: "green",
  failed: "red",
  dead: "red",
  canceled: "grey",
};

/** The tone for a `PipelineView.deploy` word. */
export function deployTone(deploy: string): Tone {
  return DEPLOY_TONES[deploy] ?? "grey";
}

/** The deploy outcome as a phrase. */
export function deployWord(deploy: string): string {
  switch (deploy) {
    case "absent":
      return "no deploy";
    case "in_progress":
      return "deploying";
    case "live":
      return "deployed";
    case "failed":
      return "deploy failed";
    case "dead":
      return "deploy never ran";
    case "canceled":
      return "deploy canceled";
    default:
      return deploy;
  }
}

/** A job class as a tone, for the sub-row dots. */
export function jobTone(jobClass: string): Tone {
  switch (jobClass) {
    case "passed":
      return "green";
    case "live":
      return "blue";
    case "blocking_failure":
      return "red";
    case "warning_failure":
      return "amber";
    case "gate":
      return "amber";
    default:
      return "grey";
  }
}

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * "4m", "2h", "3d" — an age, not a duration, so it is always in the past.
 *
 * `now` is a parameter rather than a call to `Date.now()` so the tests are not
 * timing-dependent and so a whole popover render shares one clock.
 */
export function relativeAge(iso: string | null | undefined, now: number): string {
  if (!iso) return "";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  // A clock skew of a few seconds between this machine and GitLab must not
  // produce "in 3 seconds" on a row that already exists.
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 45) return `${seconds}s`;
  // ⛔ The threshold is the ROUNDED value, not the raw one. Comparing seconds
  // against HOUR and then rounding printed "60m" for 3,599 seconds and "24h"
  // for 86,399 — a unit the next branch exists to avoid, one second before it
  // would have been used.
  const minutes = Math.round(seconds / MINUTE);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(seconds / HOUR);
  if (hours < 24) return `${hours}h`;
  return `${Math.round(seconds / DAY)}d`;
}

/** "in 12s", "now", or "" when nothing is scheduled. */
export function countdown(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "";
  if (seconds <= 0) return "now";
  return `in ${seconds}s`;
}

/** Milliseconds, rendered for the request log. */
export function millis(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${ms}ms`;
}

/**
 * A pipeline source, shortened for a badge.
 *
 * Unknown sources pass through verbatim: GitLab adds sources, and a badge that
 * says `merge_request_event` is worse than one that says nothing but better
 * than one that says the wrong thing.
 */
export function sourceBadge(source: string | null | undefined): string {
  if (!source) return "";
  switch (source) {
    case "merge_request_event":
      return "MR";
    case "schedule":
      return "sched";
    case "pipeline":
      return "child";
    case "web":
      return "web";
    case "api":
      return "api";
    case "trigger":
      return "trig";
    default:
      return source;
  }
}

/** A hostname, for the request-log rows. */
export function shortPath(path: string, max = 54): string {
  if (path.length <= max) return path;
  // Keep the tail: the query string is where the interesting part of a GitLab
  // API path lives, and the `/projects/<id>/` head is the same on every row.
  return `…${path.slice(path.length - max + 1)}`;
}

/** The HTTP status as a tone. A transport failure has no status at all. */
export function statusTone(status: number | null): Tone {
  if (status === null) return "red";
  if (status >= 500) return "red";
  if (status === 429) return "amber";
  if (status >= 400) return "red";
  if (status >= 200 && status < 300) return "green";
  return "grey";
}

/**
 * The configuration file's complaints, one line each, worded EXACTLY as the
 * shell's `poller::config_errors` words them.
 *
 * ⛔ H9: the popover's error strip used to be filled only by `Snapshot.errors`,
 * and a tick that forgot the file's errors emptied it while the file on disk
 * still would not load. `Status` (asked for every second while the popover is
 * on screen) has carried `configOk` and the diagnostics all along; this is how
 * the strip reads them. Same spelling as the shell so `errorLines` can dedupe.
 */
export function configErrorLines(
  status: Pick<Status, "configOk" | "diagnostics"> | null | undefined,
): string[] {
  if (!status || status.configOk) return [];
  const lines = status.diagnostics
    .filter((d: DiagnosticView) => d.severity === "error")
    .map((d) => {
      if (d.line !== null && d.path) return `${d.path} (line ${d.line}): ${d.message}`;
      if (d.line !== null) return `line ${d.line}: ${d.message}`;
      if (d.path) return `${d.path}: ${d.message}`;
      return d.message;
    });
  return lines.length > 0
    ? lines
    : ["the configuration file does not load; open Settings to fix it"];
}

/**
 * The start of the shell's `config::FIRST_RUN_MESSAGE`, the diagnostic a first
 * launch with no file carries. Used only until `Status.firstRun` is sent.
 */
export const FIRST_RUN_PREFIX = "No configuration yet.";

/** Whether this is a first launch with no configuration file yet. */
export function isFirstRun(status: Pick<Status, "configOk" | "diagnostics" | "firstRun"> | null | undefined): boolean {
  if (!status) return false;
  if (status.firstRun !== undefined) return status.firstRun;
  return !status.configOk && status.diagnostics.some((d) => d.message.startsWith(FIRST_RUN_PREFIX));
}

/** Everything the error strip shows: the file's complaints first, no line twice. */
export function errorLines(
  errors: readonly string[],
  status: Pick<Status, "configOk" | "diagnostics"> | null | undefined,
): string[] {
  const out = configErrorLines(status);
  for (const e of errors) if (!out.includes(e)) out.push(e);
  return out;
}

/**
 * Tailwind classes per tone, for the popover and Settings.
 *
 * ⚠ Literal strings in full, never `text-tone-${tone}`: Tailwind generates only
 * the class names it can find verbatim in the source.
 */
export const TONE_TEXT: Record<Tone, string> = {
  red: "text-tone-red",
  amber: "text-tone-amber",
  green: "text-tone-green",
  blue: "text-tone-blue",
  grey: "text-tone-grey",
};

export const TONE_BG: Record<Tone, string> = {
  red: "bg-tone-red-bg",
  amber: "bg-tone-amber-bg",
  green: "bg-tone-green-bg",
  blue: "bg-tone-blue-bg",
  grey: "bg-tone-grey-bg",
};

export const TONE_DOT: Record<Tone, string> = {
  red: "bg-tone-red",
  amber: "bg-tone-amber",
  green: "bg-tone-green",
  blue: "bg-tone-blue",
  grey: "bg-tone-grey",
};
