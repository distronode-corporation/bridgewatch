/**
 * The wizard's pure logic: step list, client-side validation, and the
 * mapping from what the form holds to `WizardAnswers`.
 *
 * The validation mirrors `bridgewatch_core::wizard::validate_answers` so a
 * user hears about a problem on the page that has it, before any request.
 * The core validates again on preview and save and is the authority; its
 * `StepIssue`s are shown on the page they name.
 */

import type { NotifyAnswers, ProjectRef, Provider, TokenSource, WizardAnswers, WizardStepId } from "./api";

export type StepId = WizardStepId | "review";

export interface StepInfo {
  id: StepId;
  title: string;
  /** Short label for the stepper. */
  label: string;
}

export const STEPS: readonly StepInfo[] = [
  { id: "account", title: "Connect to GitLab", label: "Account" },
  { id: "project", title: "Choose a project", label: "Project" },
  { id: "watch", title: "What to watch", label: "Watch" },
  { id: "deploy", title: "Deploy detection", label: "Deploy" },
  { id: "preferences", title: "Notifications and polling", label: "Preferences" },
  { id: "review", title: "Review config.toml", label: "Review" },
];

/** The account step's title, which names the provider being connected to. */
export function stepTitle(step: StepInfo, provider: Provider): string {
  if (step.id === "account" && provider === "github") return "Connect to GitHub";
  return step.title;
}

/**
 * Where the token comes from. `cli` is the provider's own CLI keyring item:
 * glab's for GitLab, gh's for GitHub (its radio carries the CLI's name).
 */
export type TokenMode = "cli" | "paste" | "env" | "command";

/** The CLI whose keyring item the `cli` token mode reads. */
export function cliName(provider: Provider): "glab" | "gh" {
  return provider === "github" ? "gh" : "glab";
}

/**
 * The keyring service the provider's CLI keeps a host's token under, with an
 * EMPTY user in both cases: `glab:<host>:token` (`glab_service_for` in the
 * core) and `gh:<web host>` (`gh_service_for`), where the empty-user item is
 * gh's active-account slot, the one `gh auth switch` moves. A GitHub API host
 * loses one leading `api.`, because gh names the WEB host. Null without a host.
 */
export function cliKeyringService(provider: Provider, baseUrl: string): string | null {
  const match = /^https?:\/\/([^/]+)/i.exec(baseUrl.trim());
  if (!match) return null;
  const host = match[1].toLowerCase();
  if (provider === "github") {
    const web = host.startsWith("api.") ? host.slice(4) : host;
    return web ? `gh:${web}` : null;
  }
  return `glab:${host}:token`;
}

/** `Provider::default_base_url` in the core: the hosted service's API root. */
export function hostedBaseUrl(provider: Provider): string {
  return provider === "github" ? "https://api.github.com" : "https://gitlab.com";
}

/**
 * The live poll interval a new watch starts at: `GITHUB_LIVE_SECS` in the
 * core for GitHub, the schema default for GitLab. GitHub's budget is 5,000
 * requests an HOUR per token, about 24 times less than gitlab.com's, so a
 * GitHub watch starts at 30 s rather than 5.
 */
export function defaultLiveSecs(provider: Provider): number {
  return provider === "github" ? 30 : 5;
}

/** The GitLab pipeline sources the watch step offers. The core knows more; these are the useful ones. */
export const SOURCE_CHOICES: readonly { value: string; label: string }[] = [
  { value: "push", label: "Push" },
  { value: "merge_request_event", label: "Merge requests" },
  { value: "web", label: "Run from the web UI" },
  { value: "schedule", label: "Scheduled" },
  { value: "api", label: "API" },
  { value: "trigger", label: "Trigger token" },
];

/** The GitHub workflow events the watch step offers, all in the core's `KNOWN_GITHUB_EVENTS`. */
export const GITHUB_SOURCE_CHOICES: readonly { value: string; label: string }[] = [
  { value: "push", label: "Push" },
  { value: "pull_request", label: "Pull requests" },
  { value: "schedule", label: "Scheduled" },
  { value: "workflow_dispatch", label: "Run manually (workflow_dispatch)" },
  { value: "release", label: "Release" },
  { value: "merge_group", label: "Merge queue" },
];

export function sourceChoices(provider: Provider): readonly { value: string; label: string }[] {
  return provider === "github" ? GITHUB_SOURCE_CHOICES : SOURCE_CHOICES;
}

export const LIVE_SPEEDS: readonly { value: number; label: string }[] = [
  { value: 5, label: "Every 5 s (default)" },
  { value: 10, label: "Every 10 s" },
  { value: 20, label: "Every 20 s" },
  { value: 30, label: "Every 30 s" },
  { value: 60, label: "Every 60 s" },
];

/** GitHub's choices: nothing faster than 10 s, and 30 s marked as its default. */
export const GITHUB_LIVE_SPEEDS: readonly { value: number; label: string }[] = [
  { value: 10, label: "Every 10 s" },
  { value: 20, label: "Every 20 s" },
  { value: 30, label: "Every 30 s (default)" },
  { value: 60, label: "Every 60 s" },
];

export function liveSpeeds(provider: Provider): readonly { value: number; label: string }[] {
  return provider === "github" ? GITHUB_LIVE_SPEEDS : LIVE_SPEEDS;
}

/** `TOKEN_PREFIXES` in the core. Anything carrying one is a token, not a pointer to one. */
export const TOKEN_PREFIXES = [
  "glpat-",
  "glptt-",
  "gldt-",
  "glrt-",
  "glrtr-",
  "glcbt-",
  "glsoat-",
  "glffct-",
  "glimt-",
  "glagent-",
  "gloas-",
  "glft-",
  "glsa-",
  "go-keyring-base64:",
  "go-keyring-encoded:",
] as const;

/**
 * `GITHUB_TOKEN_PREFIXES` in the core, checked only for a GitHub account (as
 * the core does), so a GitLab answer is judged exactly as it always was.
 */
export const GITHUB_TOKEN_PREFIXES = ["ghp_", "github_pat_", "gho_", "ghu_", "ghs_", "ghr_"] as const;

export function findTokenPrefix(text: string, provider: Provider = "gitlab"): string | null {
  const own = TOKEN_PREFIXES.find((prefix) => text.includes(prefix));
  if (own) return own;
  if (provider !== "github") return null;
  return GITHUB_TOKEN_PREFIXES.find((prefix) => text.includes(prefix)) ?? null;
}

/** The form's state, flat and serialisable. */
export interface Draft {
  /** Which CI provider. GitLab first and the default, so its path is unchanged. */
  provider: Provider;
  account: string;
  /** False until the user edits the account name; until then it follows the URL. */
  accountEdited: boolean;
  /** `hosted` is gitlab.com or github.com; `self-managed` a GitLab instance or GitHub Enterprise. */
  instance: "hosted" | "self-managed";
  selfManagedUrl: string;
  tokenMode: TokenMode;
  /** The provider CLI's keyring source (glab's or gh's), when detection found one. */
  cliSource: TokenSource | null;
  /** Pasted token. Lives only in memory, goes only to the shell as `secret`. */
  secret: string;
  /** Re-running on a config that already uses bridgewatch's own keyring entry: an empty paste keeps it. */
  ownStored: boolean;
  envVar: string;
  commandText: string;

  /** The chosen project. `ref` is what gets written: the id once resolved. */
  project: { ref: ProjectRef; path: string; default_branch: string | null } | null;
  projectInput: string;

  refName: string;
  /** GitHub only: the workflow file name or id. Empty is every workflow. */
  workflow: string;
  sources: string[];
  watchId: string;
  watchIdEdited: boolean;
  scheduleWatch: boolean;
  preflightEnabled: boolean;
  preflightRef: string;

  markers: string[];
  noMarker: boolean;

  notify: NotifyAnswers;
  launchAtLogin: boolean;
  liveSecs: number;
}

export function emptyDraft(provider: Provider = "gitlab"): Draft {
  return {
    provider,
    account: provider,
    accountEdited: false,
    instance: "hosted",
    selfManagedUrl: "",
    tokenMode: "paste",
    cliSource: null,
    secret: "",
    ownStored: false,
    envVar: "",
    commandText: "",
    project: null,
    projectInput: "",
    refName: "main",
    workflow: "",
    sources: ["push"],
    watchId: "main",
    watchIdEdited: false,
    scheduleWatch: false,
    preflightEnabled: false,
    preflightRef: "",
    markers: [],
    noMarker: false,
    notify: { deployed: true, blocking_failure: true, finished: false },
    launchAtLogin: false,
    liveSecs: defaultLiveSecs(provider),
  };
}

/**
 * Switch the draft to another provider. What cannot carry over is reset
 * (the instance, a detected CLI source, a chosen project, sources outside the
 * new vocabulary, a preflight watch on GitHub, a speed still at the old
 * default); what the user typed that still means something is kept.
 */
export function switchProvider(draft: Draft, provider: Provider): void {
  if (draft.provider === provider) return;
  const previous = draft.provider;
  draft.provider = provider;
  draft.instance = "hosted";
  draft.selfManagedUrl = "";
  draft.cliSource = null;
  if (draft.tokenMode === "cli") draft.tokenMode = "paste";
  if (!draft.accountEdited) draft.account = provider;
  draft.project = null;
  draft.projectInput = "";
  const known = new Set(sourceChoices(provider).map((c) => c.value));
  const kept = draft.sources.filter((s) => known.has(s));
  draft.sources = kept.length > 0 ? kept : ["push"];
  if (provider === "github") {
    draft.preflightEnabled = false;
    draft.preflightRef = "";
  } else {
    draft.workflow = "";
  }
  if (draft.liveSecs === defaultLiveSecs(previous)) draft.liveSecs = defaultLiveSecs(provider);
}

/** Prefill a draft from answers (re-running the wizard on an existing config). */
export function draftFromAnswers(answers: Partial<WizardAnswers>): Draft {
  const provider: Provider = answers.provider === "github" ? "github" : "gitlab";
  const draft = emptyDraft(provider);
  if (answers.base_url) {
    const hosted =
      provider === "github"
        ? /^https:\/\/api\.github\.com\/?$/i.test(answers.base_url.trim())
        : /^https:\/\/gitlab\.com\/?$/i.test(answers.base_url.trim());
    draft.instance = hosted ? "hosted" : "self-managed";
    draft.selfManagedUrl = hosted ? "" : answers.base_url;
  }
  if (answers.account) {
    draft.account = answers.account;
    draft.accountEdited = true;
  }
  const token = answers.token;
  if (token) {
    if ("env" in token) {
      draft.tokenMode = "env";
      draft.envVar = token.env;
    } else if ("command" in token) {
      draft.tokenMode = "command";
      draft.commandText = formatCommand(token.command);
    } else if ("keyring" in token) {
      draft.tokenMode = "cli";
      draft.cliSource = token;
    } else {
      draft.tokenMode = "paste";
      draft.ownStored = token.own;
    }
  }
  if (answers.project !== undefined && answers.project !== null) {
    draft.projectInput = String(answers.project);
    draft.project = { ref: answers.project, path: String(answers.project), default_branch: null };
  }
  if (answers.ref_name) draft.refName = answers.ref_name;
  if (answers.sources?.length) draft.sources = [...answers.sources];
  if (provider === "github" && answers.workflow) draft.workflow = answers.workflow;
  if (answers.watch_id) {
    draft.watchId = answers.watch_id;
    draft.watchIdEdited = true;
  }
  if (answers.schedule_watch !== undefined) draft.scheduleWatch = answers.schedule_watch;
  if (answers.preflight_ref) {
    draft.preflightEnabled = true;
    draft.preflightRef = answers.preflight_ref;
  }
  if (answers.deploy_markers) {
    draft.markers = [...answers.deploy_markers];
    draft.noMarker = answers.deploy_markers.length === 0;
  }
  if (answers.notify) draft.notify = { ...answers.notify };
  if (answers.launch_at_login !== undefined && answers.launch_at_login !== null) {
    draft.launchAtLogin = answers.launch_at_login;
  }
  if (answers.live_secs) draft.liveSecs = answers.live_secs;
  return draft;
}

export function baseUrlOf(draft: Draft): string {
  return draft.instance === "hosted"
    ? hostedBaseUrl(draft.provider)
    : draft.selfManagedUrl.trim().replace(/\/+$/, "");
}

/** The host of an instance URL, or null when it has none. */
function hostOf(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
    return parsed.host || null;
  } catch {
    return null;
  }
}

/**
 * `suggest_account_name`: `gitlab` for gitlab.com, `github` for github.com,
 * else the host's first label. A GitHub API host loses one leading `api.`
 * first, or api.github.com would suggest `api`.
 */
export function suggestAccountName(baseUrl: string, provider: Provider = "gitlab"): string {
  let host = (hostOf(baseUrl) ?? "").toLowerCase();
  if (provider === "github" && host.startsWith("api.")) host = host.slice(4);
  const first = host.split(/[.:]/)[0] ?? "";
  const clean = first.replace(/[^A-Za-z0-9_-]/g, "");
  return clean || provider;
}

/**
 * What a picked or resolved project is WRITTEN as: the id on GitLab (it
 * survives a rename), the `owner/repo` path on GitHub, where no endpoint takes
 * an id and the core refuses one.
 */
export function projectRefFor(provider: Provider, project: { id: number; path: string }): ProjectRef {
  return provider === "github" ? project.path : project.id;
}

/** `suggest_watch_id`: `<project>-<ref>`, lowercased, non-word runs as one `-`. */
export function suggestWatchId(projectPath: string, refName: string): string {
  const last = projectPath.split("/").pop() ?? projectPath;
  return `${last}-${refName}`
    .toLowerCase()
    .replace(/[^a-z0-9_]/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * Split a command line into argv. Whitespace separates; single or double
 * quotes group; a backslash escapes the next character outside single quotes.
 * No expansion of any kind: the argv runs without a shell.
 */
export function parseCommand(text: string): string[] {
  const argv: string[] = [];
  let current = "";
  let inWord = false;
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === quote) quote = null;
      else if (c === "\\" && quote === '"' && i + 1 < text.length) current += text[++i];
      else current += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      inWord = true;
    } else if (c === "\\" && i + 1 < text.length) {
      current += text[++i];
      inWord = true;
    } else if (/\s/.test(c)) {
      if (inWord) argv.push(current);
      current = "";
      inWord = false;
    } else {
      current += c;
      inWord = true;
    }
  }
  if (inWord) argv.push(current);
  return argv;
}

/** argv back to a readable line, quoting what needs it. */
export function formatCommand(argv: readonly string[]): string {
  return argv.map((arg) => (arg === "" || /[\s"'\\]/.test(arg) ? `'${arg.replace(/'/g, "'\\''")}'` : arg)).join(" ");
}

export function tokenSourceOf(draft: Draft): TokenSource {
  switch (draft.tokenMode) {
    case "cli":
      return draft.cliSource ?? { own: true };
    case "env":
      return { env: draft.envVar.trim() };
    case "command":
      return { command: parseCommand(draft.commandText) };
    default:
      return { own: true };
  }
}

export function projectRefOf(draft: Draft): ProjectRef | null {
  return draft.project ? draft.project.ref : null;
}

/** The answers the draft describes. Never contains the pasted token. */
export function answersOf(draft: Draft): WizardAnswers {
  const github = draft.provider === "github";
  const workflow = draft.workflow.trim();
  return {
    // Only for GitHub: absent is gitlab in the core, so a GitLab payload is
    // exactly the one it always was.
    ...(github ? { provider: "github" as const, workflow: workflow || null } : {}),
    account: draft.account.trim(),
    base_url: baseUrlOf(draft),
    token: tokenSourceOf(draft),
    project: projectRefOf(draft),
    watch_id: draft.watchId.trim(),
    ref_name: draft.refName.trim(),
    sources: [...draft.sources],
    deploy_markers: draft.noMarker ? [] : draft.markers.map((m) => m.trim()).filter((m) => m.length > 0),
    schedule_watch: draft.scheduleWatch,
    preflight_ref: draft.preflightEnabled && !github ? draft.preflightRef.trim() : null,
    notify: { ...draft.notify },
    launch_at_login: draft.launchAtLogin,
    live_secs: draft.liveSecs,
  };
}

/** Field → message. Empty means the step may be left. */
export type StepErrors = Record<string, string>;

const ENV_NAME = /^[A-Za-z0-9_]+$/;

/** Validate one step. `review` has nothing of its own to check. */
export function validateStep(step: StepId, draft: Draft): StepErrors {
  const errors: StepErrors = {};
  switch (step) {
    case "account": {
      const github = draft.provider === "github";
      if (!draft.account.trim()) errors.account = `The account needs a name, e.g. ${draft.provider}.`;
      if (draft.instance === "self-managed") {
        const url = draft.selfManagedUrl.trim();
        const example = github ? "https://github.example.com" : "https://gitlab.example.com";
        if (!url) errors.base_url = github
          ? `Enter your GitHub Enterprise address, e.g. ${example}.`
          : `Enter your GitLab's address, e.g. ${example}.`;
        else if (!hostOf(url)) errors.base_url = `"${url}" is not an instance URL; use e.g. ${example}.`;
      }
      switch (draft.tokenMode) {
        case "cli":
          if (!draft.cliSource)
            errors.token = `${cliName(draft.provider)}'s token was not found for this instance; choose another source.`;
          break;
        case "paste":
          if (!draft.secret.trim() && !draft.ownStored)
            errors.token = github
              ? "Paste a personal access token (classic or fine-grained)."
              : "Paste a personal, project or group access token.";
          break;
        case "env":
          if (!draft.envVar.trim()) errors.token = "Name the environment variable that holds the token.";
          else if (findTokenPrefix(draft.envVar, draft.provider))
            errors.token = "That looks like a token itself. Paste it with the first option; here goes only a variable NAME.";
          else if (!ENV_NAME.test(draft.envVar.trim()))
            errors.token = "A variable name is letters, digits and underscores.";
          break;
        case "command": {
          const argv = parseCommand(draft.commandText);
          if (argv.length === 0 || !argv[0].trim()) errors.token = "Enter the command that prints the token.";
          else if (findTokenPrefix(draft.commandText, draft.provider))
            errors.token = "That looks like a token itself. The command must PRINT the token, not contain it.";
          break;
        }
      }
      break;
    }
    case "project":
      if (!draft.project)
        errors.project =
          draft.provider === "github"
            ? "Pick a repository, or type owner/repo or its URL and look it up."
            : "Pick a project, or type its id, path or URL and look it up.";
      break;
    case "watch": {
      if (!draft.refName.trim()) errors.ref_name = "Name the branch to watch.";
      if (!draft.watchId.trim()) errors.watch_id = "The watch needs an id.";
      else if (!/^[A-Za-z0-9_-]+$/.test(draft.watchId.trim()))
        errors.watch_id = "A watch id is letters, digits, - and _.";
      if (draft.sources.length === 0)
        errors.sources =
          draft.provider === "github" ? "Choose at least one event." : "Choose at least one pipeline source.";
      if (draft.provider !== "github" && draft.preflightEnabled && !draft.preflightRef.trim())
        errors.preflight_ref = "Enter a branch pattern, e.g. pf/*, or turn preflight watching off.";
      break;
    }
    case "deploy":
      if (!draft.noMarker && draft.markers.every((m) => !m.trim()))
        errors.deploy_markers = "Pick a deploy job, or choose \u201cNo deploy marker\u201d.";
      break;
    case "preferences":
      if (!(draft.liveSecs >= 1 && draft.liveSecs <= 60))
        errors.live_secs = "The live poll interval is between 1 and 60 seconds.";
      break;
    case "review":
      break;
  }
  return errors;
}
