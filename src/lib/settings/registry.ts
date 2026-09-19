/**
 * Every configuration key the settings pane can edit, and how.
 *
 * This is the list the parity test walks. A key that exists in
 * `config.schema.json` and not here means the settings pane silently cannot
 * edit part of the file; a key here and not in the schema means a control
 * writes something the core will reject. Both fail `npm test` with the path
 * named, which is the only reason this file is an explicit array rather than
 * something derived at runtime: a derived registry would agree with the schema
 * by construction and prove nothing.
 *
 * Paths use the same spelling as the schema walk:
 *   * `accounts.*`         a dictionary table, one wildcard segment
 *   * `watches.*`          an array of tables
 *   * `watches.*.jobs.*`   a dictionary whose keys are the user's patterns, so
 *                          the whole table is ONE leaf, edited as ordered rows
 *
 * A control renders and writes exactly one of these; nothing in a Svelte
 * component may invent a path.
 */

/** Which settings tab a key lives on. */
export type Tab = "accounts" | "watches" | "icon" | "verdict" | "ui" | "log";

/**
 * How a key is edited.
 *
 * `token` and `project` are composites rather than a plain field because the
 * underlying schema node is a union: `accounts.*.token` is one of four table
 * shapes, and `watches.*.project` is a number or a path.
 */
export type Control =
  | "text"
  | "textarea"
  | "number"
  | "boolean"
  | "select"
  | "list"
  | "token"
  | "project"
  | "jobs"
  | "states";

/**
 * What an emptied text control means for this key.
 *
 * ⛔ The two answers are not interchangeable and the wrong one is dangerous.
 * `unset` removes the key, so the core's default comes back — which is right
 * for a key whose default is "off" and WRONG for `dive.bridges`, whose default
 * is `"*"`: clearing a field hinted `"" is none` reinstated a dive into every
 * bridge of every pipeline, i.e. the opposite of what was asked, at a cost of
 * one API call per bridge per tick.
 */
export type EmptyMeans = "unset" | "empty";

/** One editable key. */
export interface RegistryEntry {
  /** Dotted path, wildcards as `*`. */
  path: string;
  /** Which tab renders it. */
  tab: Tab;
  /** Which control renders it. */
  control: Control;
  /** The field label. */
  label: string;
  /** One line under the field. Kept short: the config file has the prose. */
  hint?: string;
  /** For `select`, the allowed values in the order they are offered. */
  options?: string[];
  /**
   * What clearing the control writes. Defaults to `unset`, which is right for
   * every key whose default is absence; say `empty` where the empty string is
   * itself a value the user can mean.
   */
  emptyMeans?: EmptyMeans;
}

export const REGISTRY: RegistryEntry[] = [
  // --- Accounts ----------------------------------------------------------
  {
    path: "accounts.*.provider",
    tab: "accounts",
    control: "select",
    label: "Provider",
    options: ["gitlab", "github"],
    // Changing it changes what the other account fields mean: the base URL, the
    // API path and the auth header all default per provider, and a github
    // account reads `Authorization: Bearer` and nothing else.
    hint: "GitLab CI, or GitHub Actions. The base URL, API path and auth header follow it.",
  },
  {
    path: "accounts.*.base_url",
    tab: "accounts",
    control: "text",
    label: "Base URL",
    hint: "Instance root, no trailing slash. Self-managed instances welcome.",
  },
  {
    path: "accounts.*.api_path",
    tab: "accounts",
    control: "text",
    label: "API path",
    hint: "Overridable for proxies that mount the API elsewhere.",
  },
  {
    path: "accounts.*.token",
    tab: "accounts",
    control: "token",
    label: "Token source",
    hint: "Never the token itself. bridgewatch will not store a credential in this file.",
  },
  {
    path: "accounts.*.header",
    tab: "accounts",
    control: "select",
    label: "Auth header",
    options: ["PRIVATE-TOKEN", "Authorization: Bearer"],
  },
  {
    path: "accounts.*.timeout_secs",
    tab: "accounts",
    control: "number",
    label: "Timeout (s)",
  },
  {
    path: "accounts.*.rate_limit_backoff.max_secs",
    tab: "accounts",
    control: "number",
    label: "Backoff ceiling (s)",
    hint: "How far the poll interval may grow after rate limiting.",
  },

  // --- Watches -----------------------------------------------------------
  { path: "watches.*.id", tab: "watches", control: "text", label: "Id" },
  { path: "watches.*.account", tab: "watches", control: "select", label: "Account" },
  {
    path: "watches.*.project",
    tab: "watches",
    control: "project",
    label: "Project",
    hint: "Numeric id, or group/path which is URL-encoded for you.",
  },
  {
    path: "watches.*.ref",
    tab: "watches",
    control: "text",
    label: "Ref",
    hint: 'Exact (main), glob (pf/*) or regex (re:^release/.*$).',
  },
  {
    path: "watches.*.workflow",
    tab: "watches",
    control: "text",
    label: "Workflow",
    hint: "GitHub only: one workflow file (ci.yml) or id. Empty watches every workflow.",
  },
  {
    path: "watches.*.sources",
    tab: "watches",
    control: "list",
    label: "Sources",
    hint: "Pipeline sources to accept. Empty means all of them.",
  },
  {
    path: "watches.*.role",
    tab: "watches",
    control: "select",
    label: "Role",
    options: ["primary", "secondary"],
    hint: "Primary drives the tray icon and may notify. Secondary is rows only.",
  },
  { path: "watches.*.show.max_rows", tab: "watches", control: "number", label: "Max rows" },
  {
    path: "watches.*.show.settled",
    tab: "watches",
    control: "number",
    label: "Settled rows",
    hint: "Unsettled pipelines are always shown; this is how many finished ones join them.",
  },
  {
    path: "watches.*.show.jobs",
    tab: "watches",
    control: "select",
    label: "Jobs shown",
    // "" is "inherit": the key is removed and `ui.jobs` decides.
    options: ["", "failures", "all"],
    hint: "Empty follows the UI tab's setting.",
  },
  { path: "watches.*.poll.live_secs", tab: "watches", control: "number", label: "Poll while live (s)" },
  { path: "watches.*.poll.idle_secs", tab: "watches", control: "number", label: "Poll while idle (s)" },
  {
    path: "watches.*.dive.bridges",
    tab: "watches",
    control: "text",
    label: "Dive into bridges",
    hint: 'Glob over trigger-job names. "*" is all of them, "" is none.',
    // ⛔ The default is `"*"`. Removing the key to mean "none" would turn the
    // dive back on for every bridge, which is what the hint promises it stops.
    emptyMeans: "empty",
  },
  { path: "watches.*.dive.exclude", tab: "watches", control: "list", label: "Dive exclusions" },
  { path: "watches.*.dive.depth", tab: "watches", control: "number", label: "Dive depth" },
  {
    path: "watches.*.dive.only_when",
    tab: "watches",
    control: "text",
    label: "Dive only when",
    hint: 'A bridge status, e.g. "failed". Keeps a noisy secondary watch cheap.',
  },
  {
    path: "watches.*.deploy_markers",
    tab: "watches",
    control: "list",
    label: "Deploy markers",
    hint: 'Job names or "re:" patterns. The first one listed that has a success means deployed.',
  },
  {
    path: "watches.*.sibling_failure",
    tab: "watches",
    control: "select",
    label: "Sibling failure",
    options: ["downgrade", "fail", "ignore"],
  },
  {
    path: "watches.*.post_deploy_failure",
    tab: "watches",
    control: "select",
    label: "Post-deploy failure",
    options: ["downgrade", "fail", "ignore"],
  },
  {
    path: "watches.*.jobs.*",
    tab: "watches",
    control: "jobs",
    label: "Job class overrides",
    hint: "Matched in file order, first wins — so the rows' order is the rule.",
  },
  { path: "watches.*.notify.deployed", tab: "watches", control: "boolean", label: "Notify: deployed" },
  {
    path: "watches.*.notify.blocking_failure",
    tab: "watches",
    control: "boolean",
    label: "Notify: blocking failure",
  },
  { path: "watches.*.notify.finished", tab: "watches", control: "boolean", label: "Notify: finished" },
  { path: "watches.*.notify.started", tab: "watches", control: "boolean", label: "Notify: started" },
  {
    path: "watches.*.notify.gate_opened",
    tab: "watches",
    control: "boolean",
    label: "Notify: gate opened",
  },
  {
    path: "watches.*.notify.title",
    tab: "watches",
    control: "text",
    label: "Notification title",
    hint: "MiniJinja over the verdict view model.",
  },
  {
    path: "watches.*.notify.body",
    tab: "watches",
    control: "text",
    label: "Notification body",
    hint: "default('…', true) — the second argument is load-bearing on an empty list.",
  },
  {
    path: "watches.*.notify.click",
    tab: "watches",
    control: "select",
    label: "Notification click",
    options: ["first_failure_or_pipeline", "pipeline", "marker_job", "none"],
  },

  // --- Icon --------------------------------------------------------------
  {
    path: "icon.mode",
    tab: "icon",
    control: "select",
    label: "Mode",
    options: ["auto", "template", "color"],
    hint: "auto: template on macOS, colour elsewhere.",
  },
  {
    path: "icon.theme",
    tab: "icon",
    control: "text",
    label: "Theme",
    hint: 'builtin, or a directory of <state>.png overrides.',
  },
  {
    path: "icon.states.*",
    tab: "icon",
    control: "states",
    label: "State glyphs",
    hint: "Remap a state onto another symbol: question, octagon, triangle, check, check-outline, arrows, slash, hourglass.",
  },

  // --- Verdict -----------------------------------------------------------
  {
    path: "verdict.script",
    tab: "verdict",
    control: "text",
    label: "Script path",
    hint: "A .rhai script replacing the built-in icon-state rules. ~ is expanded.",
  },
  {
    path: "verdict.script_source",
    tab: "verdict",
    control: "textarea",
    label: "Inline script",
    hint: "Takes precedence over the path. A script that errors yields unknown, never a confident wrong colour.",
  },

  // --- UI ----------------------------------------------------------------
  { path: "ui.popover.width", tab: "ui", control: "number", label: "Popover width" },
  { path: "ui.popover.max_height", tab: "ui", control: "number", label: "Popover max height" },
  {
    path: "ui.popover.hide_on_blur",
    tab: "ui",
    control: "boolean",
    label: "Hide on blur",
  },
  {
    path: "ui.theme_css",
    tab: "ui",
    control: "text",
    label: "Theme CSS",
    hint: "Path to a stylesheet appended after the built-in theme.",
  },
  {
    path: "ui.jobs",
    tab: "ui",
    control: "select",
    label: "Jobs shown",
    options: ["failures", "all"],
    hint: "Every job of a pipeline, or only the ones that decide the verdict.",
  },
  { path: "ui.launch_at_login", tab: "ui", control: "boolean", label: "Launch at login" },

  // --- Log ---------------------------------------------------------------
  {
    path: "log.level",
    tab: "log",
    control: "select",
    label: "Log level",
    options: ["error", "warn", "info", "debug", "trace"],
    hint: "RUST_LOG wins when it is set.",
  },
  {
    path: "log.keep_requests",
    tab: "log",
    control: "number",
    label: "Requests kept",
    hint: "How many recent API calls the debug pane shows.",
  },
];

/** The registry entries on one tab, in declaration order. */
export function entriesFor(tab: Tab): RegistryEntry[] {
  return REGISTRY.filter((e) => e.tab === tab);
}

/** Look one up by path. Throws rather than returning undefined: a control that
 * asks for a path the registry does not hold is a bug, not a state. */
export function entry(path: string): RegistryEntry {
  const found = REGISTRY.find((e) => e.path === path);
  if (!found) throw new Error(`no registry entry for ${path}`);
  return found;
}

/** The concrete path for a wildcard entry, e.g. `watches.*.id` at index 2. */
export function concretePath(path: string, ...keys: (string | number)[]): string {
  let i = 0;
  return path.replace(/\*/g, () => {
    const key = keys[i++];
    if (key === undefined) throw new Error(`not enough keys for ${path}`);
    // A key holding a dot has to be quoted, which is what the core's
    // `split_path` expects: `watches.0.jobs."kics-iac-sast"`.
    return typeof key === "number" || !String(key).includes(".")
      ? String(key)
      : JSON.stringify(String(key));
  });
}
