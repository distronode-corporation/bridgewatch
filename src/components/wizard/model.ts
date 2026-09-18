/**
 * The wizard's pure logic: step list, client-side validation, and the
 * mapping from what the form holds to `WizardAnswers`.
 *
 * The validation mirrors `bridgewatch_core::wizard::validate_answers` so a
 * user hears about a problem on the page that has it, before any request.
 * The core validates again on preview and save and is the authority; its
 * `StepIssue`s are shown on the page they name.
 */

import type { NotifyAnswers, ProjectRef, TokenSource, WizardAnswers, WizardStepId } from "./api";

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

export type TokenMode = "glab" | "paste" | "env" | "command";

/** The sources the watch step offers. The core knows more; these are the useful ones. */
export const SOURCE_CHOICES: readonly { value: string; label: string }[] = [
  { value: "push", label: "Push" },
  { value: "merge_request_event", label: "Merge requests" },
  { value: "web", label: "Run from the web UI" },
  { value: "schedule", label: "Scheduled" },
  { value: "api", label: "API" },
  { value: "trigger", label: "Trigger token" },
];

export const LIVE_SPEEDS: readonly { value: number; label: string }[] = [
  { value: 5, label: "Every 5 s (default)" },
  { value: 10, label: "Every 10 s" },
  { value: 20, label: "Every 20 s" },
  { value: 30, label: "Every 30 s" },
  { value: 60, label: "Every 60 s" },
];

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

export function findTokenPrefix(text: string): string | null {
  return TOKEN_PREFIXES.find((prefix) => text.includes(prefix)) ?? null;
}

/** The form's state, flat and serialisable. */
export interface Draft {
  account: string;
  /** False until the user edits the account name; until then it follows the URL. */
  accountEdited: boolean;
  instance: "gitlab.com" | "self-managed";
  selfManagedUrl: string;
  tokenMode: TokenMode;
  /** The glab keyring source, when detection found one. */
  glabSource: TokenSource | null;
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

export function emptyDraft(): Draft {
  return {
    account: "gitlab",
    accountEdited: false,
    instance: "gitlab.com",
    selfManagedUrl: "",
    tokenMode: "paste",
    glabSource: null,
    secret: "",
    ownStored: false,
    envVar: "",
    commandText: "",
    project: null,
    projectInput: "",
    refName: "main",
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
    liveSecs: 5,
  };
}

/** Prefill a draft from answers (re-running the wizard on an existing config). */
export function draftFromAnswers(answers: Partial<WizardAnswers>): Draft {
  const draft = emptyDraft();
  if (answers.base_url) {
    const gitlabCom = /^https:\/\/gitlab\.com\/?$/i.test(answers.base_url.trim());
    draft.instance = gitlabCom ? "gitlab.com" : "self-managed";
    draft.selfManagedUrl = gitlabCom ? "" : answers.base_url;
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
      draft.tokenMode = "glab";
      draft.glabSource = token;
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
  return draft.instance === "gitlab.com" ? "https://gitlab.com" : draft.selfManagedUrl.trim().replace(/\/+$/, "");
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

/** `suggest_account_name`: `gitlab` for gitlab.com, else the host's first label. */
export function suggestAccountName(baseUrl: string): string {
  const host = hostOf(baseUrl) ?? "";
  const first = host.split(/[.:]/)[0] ?? "";
  const clean = first.replace(/[^A-Za-z0-9_-]/g, "");
  return clean || "gitlab";
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
    case "glab":
      return draft.glabSource ?? { own: true };
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
  return {
    account: draft.account.trim(),
    base_url: baseUrlOf(draft),
    token: tokenSourceOf(draft),
    project: projectRefOf(draft),
    watch_id: draft.watchId.trim(),
    ref_name: draft.refName.trim(),
    sources: [...draft.sources],
    deploy_markers: draft.noMarker ? [] : draft.markers.map((m) => m.trim()).filter((m) => m.length > 0),
    schedule_watch: draft.scheduleWatch,
    preflight_ref: draft.preflightEnabled ? draft.preflightRef.trim() : null,
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
      if (!draft.account.trim()) errors.account = "The account needs a name, e.g. gitlab.";
      if (draft.instance === "self-managed") {
        const url = draft.selfManagedUrl.trim();
        if (!url) errors.base_url = "Enter your GitLab's address, e.g. https://gitlab.example.com.";
        else if (!hostOf(url)) errors.base_url = `"${url}" is not an instance URL; use e.g. https://gitlab.example.com.`;
      }
      switch (draft.tokenMode) {
        case "glab":
          if (!draft.glabSource) errors.token = "glab's token was not found for this instance; choose another source.";
          break;
        case "paste":
          if (!draft.secret.trim() && !draft.ownStored) errors.token = "Paste a personal, project or group access token.";
          break;
        case "env":
          if (!draft.envVar.trim()) errors.token = "Name the environment variable that holds the token.";
          else if (findTokenPrefix(draft.envVar))
            errors.token = "That looks like a token itself. Paste it with the first option; here goes only a variable NAME.";
          else if (!ENV_NAME.test(draft.envVar.trim()))
            errors.token = "A variable name is letters, digits and underscores.";
          break;
        case "command": {
          const argv = parseCommand(draft.commandText);
          if (argv.length === 0 || !argv[0].trim()) errors.token = "Enter the command that prints the token.";
          else if (findTokenPrefix(draft.commandText))
            errors.token = "That looks like a token itself. The command must PRINT the token, not contain it.";
          break;
        }
      }
      break;
    }
    case "project":
      if (!draft.project) errors.project = "Pick a project, or type its id, path or URL and look it up.";
      break;
    case "watch": {
      if (!draft.refName.trim()) errors.ref_name = "Name the branch to watch.";
      if (!draft.watchId.trim()) errors.watch_id = "The watch needs an id.";
      else if (!/^[A-Za-z0-9_-]+$/.test(draft.watchId.trim()))
        errors.watch_id = "A watch id is letters, digits, - and _.";
      if (draft.sources.length === 0) errors.sources = "Choose at least one pipeline source.";
      if (draft.preflightEnabled && !draft.preflightRef.trim())
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
