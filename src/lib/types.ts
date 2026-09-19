/**
 * The wire shapes, mirroring `bridgewatch-core`'s `Serialize` impls.
 *
 * These are hand-written rather than generated because there is exactly one
 * producer and the core's README documents the whole contract. What keeps them
 * honest is `__fixtures__/ca41ab28.json`, which is real `bridgewatch check
 * --json` output and is type-checked against `Snapshot` by the popover test.
 */

/** `IconState::as_str()`. The eight names are also the icon files' stems. */
export type IconState =
  | "unknown"
  | "failed"
  | "deployed_with_failure"
  | "deployed"
  | "running"
  | "canceled"
  | "parked_gate"
  | "succeeded_no_deploy";

/** `JobClass`, after `[watches.jobs]` overrides. */
export type JobClass =
  | "passed"
  | "live"
  | "blocking_failure"
  | "warning_failure"
  | "gate"
  | "not_built"
  | "unknown"
  | "ignored";

export interface JobView {
  id: number;
  name: string;
  class: JobClass;
  status: string;
  allow_failure: boolean;
  stage: string | null;
  web_url: string | null;
  /**
   * When the job started (RFC 3339). Null for a job that never ran. A running
   * job's elapsed time ticks locally from this between polls.
   * Always sent by the core; optional only so recordings made before
   * 2026-09-18 (e.g. `__fixtures__/ca41ab28.json`) still type-check.
   */
  started_at?: string | null;
  /** When the job finished. Null while running and for a job that never ran. */
  finished_at?: string | null;
  /**
   * Seconds the job ran: GitLab's own `duration` when sent, else
   * `finished_at - started_at`. Null (never 0) for a job that never started;
   * for a running job it is GitLab's elapsed time at the last poll, if any.
   */
  duration?: number | null;
}

/** `JobsMode`: which jobs an expanded row lists. Presentation only. */
export type JobsMode = "failures" | "all";

export interface BridgeView {
  name: string;
  status: string;
  /** `passed | passed_with_warnings | failed | dead | running | awaiting_gate | canceled | skipped | unknown` */
  verdict: string;
  verdict_jobs: string[];
  child_id: number | null;
  child_url: string | null;
  child_project_id: number | null;
  /** False means nobody looked, NOT that the child had no jobs. */
  dived: boolean;
  jobs: JobView[];
  web_url: string | null;
}

export interface MarkerJob {
  name: string;
  id: number;
  pipeline_id: number;
  web_url: string | null;
  started_at: string | null;
}

export interface PipelineView {
  id: number;
  iid: number | null;
  sha: string;
  sha7: string;
  ref: string;
  source: string | null;
  status: string;
  web_url: string | null;
  state: IconState;
  /** `absent | in_progress | live | failed | dead | canceled` */
  deploy: string;
  deploy_marker: MarkerJob | null;
  deploy_failures: string[];
  failures: string[];
  warnings: string[];
  gates: string[];
  post_deploy_failures: string[];
  sibling_failures: string[];
  bridges: BridgeView[];
  parent_jobs: JobView[];
  updated_at: string | null;
  created_at: string | null;
  /** A gate is NOT live. */
  live: boolean;
}

export interface WatchView {
  id: string;
  role: "primary" | "secondary";
  /** Always null for a secondary watch, which never touches the tray. */
  icon_state: IconState | null;
  rows: PipelineView[];
  error: string | null;
  /**
   * The effective jobs mode: the watch's `show.jobs`, else `ui.jobs`, resolved
   * by the core. The rows carry every job either way. Always sent by the core;
   * optional only for older recordings, where absent means `"all"`.
   */
  jobs?: JobsMode;
  /**
   * The provider of the watch's account. The core sends it only for GitHub,
   * so a GitLab watch's JSON is byte-identical to before the key existed:
   * absent means `"gitlab"`.
   */
  provider?: Provider;
}

export interface RequestLog {
  method: string;
  path: string;
  status: number | null;
  ms: number;
  ratelimit_remaining: number | null;
  ratelimit_reset: number | null;
  retry_after: number | null;
  error: string | null;
  at: string;
}

export interface Snapshot {
  icon_state: IconState;
  watches: WatchView[];
  errors: string[];
  last_poll: string;
  request_log: RequestLog[];
}

/** One problem in a configuration file, with its position resolved. */
export interface DiagnosticView {
  severity: "error" | "warning";
  path: string;
  message: string;
  line: number | null;
  col: number | null;
}

/** A sensitive write the shell parked until the user confirms it. */
export interface ConfirmRequest {
  /** Send back with the same request to go ahead. Good for one attempt. */
  id: string;
  /** One sentence per sensitive change, worded by the shell. */
  changes: string[];
}

/** The answer to "would this text load?". */
export interface Validation {
  ok: boolean;
  diagnostics: DiagnosticView[];
  /** Present when the text is valid but was NOT written: confirm first. */
  confirm?: ConfirmRequest;
  /** Present and true when the file changed since the caller read it. */
  conflict?: boolean;
}

/** Shell-level status, distinct from the core's `Snapshot`. */
export interface Status {
  configPath: string;
  /**
   * True when this run wrote the shipped example. Nothing sets it any more
   * (the setup wizard replaced seeding) and nothing here reads it: kept only
   * because the shell still sends it, and the wire shape is the shell's.
   */
  seeded: boolean;
  configOk: boolean;
  diagnostics: DiagnosticView[];
  sinceLastPollSecs: number | null;
  nextPollSecs: number | null;
  launchAtLogin: boolean;
  platform: string;
  /** True when the popover has a native vibrancy material behind it (macOS). */
  vibrancy: boolean;
  /**
   * True on a first launch with no config file yet (the wizard's moment).
   * Optional: requested from shell-rust; until it is sent the popover
   * recognises the shell's first-run diagnostic instead.
   */
  firstRun?: boolean;
}

/** `bridgewatch_core::config::edit::Edit`, tagged by `op`. */
export type EditValue =
  | { string: string }
  | { integer: number }
  | { float: number }
  | { boolean: boolean }
  | { array: EditValue[] };

export type Edit =
  | { op: "set"; path: string; value: EditValue }
  | { op: "unset"; path: string }
  | { op: "remove_watch"; id: string }
  | { op: "set_watch"; id: string; path: string; value: EditValue }
  | { op: "add_watch"; watch: Record<string, unknown> };

/** `Provider`: which CI provider an account talks to. An account without the key is `gitlab`. */
export type Provider = "gitlab" | "github";
