/**
 * The first-run wizard's contract with the shell.
 *
 * The wizard UI never talks to Tauri itself: the shell injects a `WizardApi`
 * (backed by its commands, which call `bridgewatch_core::wizard`), and the
 * tests inject a fake. The wire shapes below mirror the core's serde output in
 * `crates/bridgewatch-core/src/wizard.rs` and `config/schema.rs`; keep them in
 * step with it.
 *
 * ⛔ A pasted token travels ONLY as `secret` on a request, from the input
 * straight to the shell, which stores it in bridgewatch's own keyring entry.
 * It is never part of `WizardAnswers`, never in the preview and never
 * rendered back.
 */

import type { ConfirmRequest, DiagnosticView, Provider, Validation } from "../../lib/types";

// ---------------------------------------------------------------------------
// Wire shapes (core mirrors)
// ---------------------------------------------------------------------------

/** `Provider`: which CI provider an account talks to. Absent on the wire is `gitlab`. */
export type { Provider } from "../../lib/types";

/** `WizardStep`: which page an issue sends the user back to. */
export type WizardStepId = "account" | "project" | "watch" | "deploy" | "preferences";

/** `StepIssue`. */
export interface StepIssue {
  step: WizardStepId;
  /** `account | base_url | token | project | watch_id | ref_name | workflow | sources | preflight_ref | deploy_markers | live_secs` */
  field: string;
  message: string;
}

/** `TokenSource`, externally tagged. Never a token. The wizard only ever sends `{ own: true }`. */
export type TokenSource =
  | { keyring: { service: string; user: string } }
  | { env: string }
  | { command: string[] }
  | { own: boolean };

/** `ProjectRef`, untagged: a numeric id or a `group/project` (GitHub: `owner/repo`) path. */
export type ProjectRef = number | string;

/** `TokenKind`, tagged by `kind`. */
export type TokenKind =
  | { kind: "personal" }
  | { kind: "project"; project_id: number | null }
  | { kind: "group"; group_id: number | null }
  | { kind: "service_account" }
  | { kind: "unknown" };

/** `Identity`: who a token authenticates as. Never carries the token. */
export interface Identity {
  username: string;
  name: string | null;
  bot: boolean;
  token: TokenKind;
  scopes: string[];
  expires_at: string | null;
  warnings: string[];
}

/** `CliTokenDetection`, tagged by `status`: glab's or gh's keyring item. Presence only, never a value. */
export type CliTokenDetection =
  | { status: "found"; source: TokenSource }
  | { status: "not_found"; service: string }
  | { status: "unavailable"; reason: string };

/** `ProjectSummary`. */
export interface ProjectSummary {
  id: number;
  path: string;
  name: string | null;
  default_branch: string | null;
  web_url: string | null;
}

/** `ProjectListing`, tagged by `mode`. */
export type ProjectListing =
  | { mode: "projects"; projects: ProjectSummary[]; truncated: boolean }
  | { mode: "type_id_or_path"; reason: string; suggestion: number | null };

/** `ResolvedProject`. */
export interface ResolvedProject {
  id: number;
  path: string;
  /**
   * What to write as the watch's `project`: the id on GitLab (it survives a
   * rename), the `owner/repo` path on GitHub (no endpoint takes an id).
   * Optional only so an older shell's answer still reads; absent means `id`.
   */
  project?: ProjectRef;
  default_branch: string | null;
  web_url: string | null;
}

/** `MarkerSuggestion`. */
export interface MarkerSuggestion {
  name: string;
  stage: string | null;
  /** `"parent"` or the trigger job whose child holds it. */
  pipeline: string;
  status: string;
  score: number;
  reasons: string[];
}

/** `MarkerSuggestions`. */
export interface MarkerSuggestions {
  pipeline_id: number | null;
  pipeline_url: string | null;
  suggestions: MarkerSuggestion[];
  unread_children: string[];
}

/** `NotifyAnswers`. */
export interface NotifyAnswers {
  deployed: boolean;
  blocking_failure: boolean;
  finished: boolean;
}

/**
 * `WizardAnswers`: everything the wizard collected. Never a token.
 *
 * `provider` and `workflow` are sent only for GitHub: absent is `gitlab` and
 * no workflow in the core, so a GitLab payload is exactly what it always was.
 */
export interface WizardAnswers {
  provider?: Provider;
  account: string;
  base_url: string;
  token: TokenSource;
  project: ProjectRef | null;
  watch_id: string;
  ref_name: string;
  /** GitHub only: the workflow file name (`ci.yml`) or id; absent or null is every workflow. */
  workflow?: string | null;
  sources: string[];
  deploy_markers: string[];
  schedule_watch: boolean;
  preflight_ref: string | null;
  notify: NotifyAnswers | null;
  launch_at_login: boolean | null;
  live_secs: number | null;
}

/** `BuiltConfig`, with diagnostics as the shell renders them. */
export interface WizardPreview {
  /** The complete config.toml that `save` would write. */
  toml: string;
  warnings: DiagnosticView[];
  /** True when an existing file is being edited rather than started. */
  edited_existing: boolean;
}

/** `FailureKind`: what a rejected request failed on. */
export type FailureKind =
  | "unauthorized"
  | "not_found"
  | "network"
  | "answers"
  | "existing_config"
  | "invalid"
  | "token_in_config";

/** The core's `Diagnostic` as serialised (`span` is a byte range, not a line). */
export interface CoreDiagnostic {
  severity: "error" | "warning";
  path: string;
  message: string;
  span: { start: number; end: number } | null;
}

/**
 * `WizardFailure`: what every api method REJECTS with. `issues` is filled
 * when `kind` is `answers` (the wizard sends the user to the first step
 * named), `diagnostics` when it is `invalid` (shown on the review step).
 */
export interface WizardFailure {
  kind: FailureKind;
  message: string;
  issues: StepIssue[];
  diagnostics: CoreDiagnostic[];
}

// ---------------------------------------------------------------------------
// The injected API
// ---------------------------------------------------------------------------

/** How to reach the instance for the live steps (1, 2 and 4). */
export interface Connection {
  /** Sent only for GitHub; the shell reads absent as `gitlab`. */
  provider?: Provider;
  base_url: string;
  token: TokenSource;
  /** A pasted token, for `{ own: true }` before it has been stored. */
  secret?: string;
  /** The id of a `ConfirmRequest` the user accepted (running a command source). */
  confirm?: string;
}

/**
 * The shell's answer when a request needs the user's say-so first (Lo13: a
 * `command` token source runs a program). Show `changes`, then repeat the
 * same request with `confirm: confirm.id`.
 */
export interface NeedsConfirm {
  confirm: ConfirmRequest;
}

export type Confirmable<T> = T | NeedsConfirm;

/** The result of `save`: the shell's `Validation`, plus answers the core refused. */
export interface WizardSaveResult extends Validation {
  /** Per-step problems from `validate_answers`; the wizard jumps to the first. */
  issues?: StepIssue[];
  /** Where the file was written, when it was. */
  path?: string;
}

/**
 * Errors: reject with a `WizardFailure` (an `Error`, or anything with a
 * `message`, is accepted too). A rejection carrying `issues` sends the user to
 * the step at fault; `diagnostics` are listed on the review step.
 */
export interface WizardApi {
  /** Is the provider CLI's keyring item (glab's or gh's) present for this instance? Existence only. Optional. */
  detectCliToken?(baseUrl: string, provider: Provider): Promise<CliTokenDetection>;
  /** Step 1: who is this token? */
  testConnection(connection: Connection): Promise<Confirmable<Identity>>;
  /** Step 2: the projects this token can pick from. */
  listProjects(connection: Connection, search?: string): Promise<Confirmable<ProjectListing>>;
  /** Step 2: resolve a typed id, path or pasted project URL. */
  resolveProject(connection: Connection, idOrPath: string): Promise<Confirmable<ResolvedProject>>;
  /**
   * Step 4: deploy-marker suggestions from the ref's latest pipeline. On
   * GitHub `workflow` narrows it to the run the watch will follow; it is
   * passed only when there is one.
   */
  suggestDeployMarkers(
    connection: Connection,
    project: ProjectRef,
    refName: string,
    workflow?: string,
  ): Promise<Confirmable<MarkerSuggestions>>;
  /** Step 6: the config.toml the answers produce (editing an existing file in place). */
  previewConfig(answers: WizardAnswers): Promise<WizardPreview>;
  /**
   * Write it. `secret` is the pasted token for `{ own: true }`; `confirm` the
   * id of a ConfirmRequest the user accepted. May answer with `confirm` set.
   */
  save(answers: WizardAnswers, options: { secret?: string; confirm?: string }): Promise<WizardSaveResult>;
  /** "I'll edit config.toml": leave the wizard, writing nothing. */
  skip(): Promise<void>;
}

/** Narrow a `Confirmable`. */
export function needsConfirm<T>(value: Confirmable<T>): value is NeedsConfirm {
  return (
    typeof value === "object" &&
    value !== null &&
    "confirm" in value &&
    typeof (value as NeedsConfirm).confirm === "object" &&
    (value as NeedsConfirm).confirm !== null &&
    typeof (value as NeedsConfirm).confirm.id === "string"
  );
}

/** Pull `StepIssue[]` out of a rejection, if it carries any. */
export function issuesOf(error: unknown): StepIssue[] {
  if (typeof error === "object" && error !== null && Array.isArray((error as { issues?: unknown }).issues)) {
    return (error as { issues: StepIssue[] }).issues;
  }
  return [];
}

/** Pull load diagnostics out of a rejection (`WizardFailure.diagnostics`), as the review step renders them. */
export function diagnosticsOf(error: unknown): DiagnosticView[] {
  if (typeof error !== "object" || error === null) return [];
  const list = (error as { diagnostics?: unknown }).diagnostics;
  if (!Array.isArray(list)) return [];
  return list
    .filter((d): d is CoreDiagnostic => typeof d === "object" && d !== null && typeof d.message === "string")
    .map((d) => ({
      severity: d.severity === "warning" ? "warning" : "error",
      path: typeof d.path === "string" ? d.path : "",
      message: d.message,
      line: null,
      col: null,
    }));
}

/** A rejection's message, for display. */
export function messageOf(error: unknown): string {
  if (typeof error === "string") return error;
  if (typeof error === "object" && error !== null && typeof (error as { message?: unknown }).message === "string") {
    return (error as { message: string }).message;
  }
  return "Something went wrong.";
}
