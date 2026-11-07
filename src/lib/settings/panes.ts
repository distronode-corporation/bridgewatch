/**
 * The Settings window's panes, and the groups a pane's fields are shown in.
 *
 * Five panes in a sidebar (macOS, Windows and GNOME all shape Settings this
 * way): Accounts, Watches, General (the icon and the app), Verdict, and
 * Advanced (the log and the file as text). The registry's `Tab` stays what it
 * was, the place a KEY lives; this file says where a tab is SHOWN.
 */

import type { Tab } from "./registry";

export type Pane = "accounts" | "watches" | "general" | "verdict" | "advanced";

export const PANES: { id: Pane; label: string }[] = [
  { id: "accounts", label: "Accounts" },
  { id: "watches", label: "Watches" },
  { id: "general", label: "General" },
  { id: "verdict", label: "Verdict" },
  { id: "advanced", label: "Advanced" },
];

/** What the shell or the last session asked for: a pane, and the part of it to show. */
export interface PaneRequest {
  pane: Pane;
  /** `"text"`: the Advanced pane's "Edit as text" editor, scrolled into view. */
  focus: "text" | null;
}

/**
 * The pane for a name the shell sends.
 *
 * ⛔ The shell still names the OLD tabs: a file that does not load, and the
 * wizard's "I'll edit config.toml", open Settings with `"text"`. Those names
 * are kept working here rather than renamed in Rust, so an older shell and
 * this window still agree. Anything unknown is null, and the caller keeps the
 * pane it has.
 */
export function paneFor(name: string | null | undefined): PaneRequest | null {
  switch (name) {
    case "accounts":
    case "watches":
    case "general":
    case "verdict":
    case "advanced":
      return { pane: name, focus: null };
    case "icon":
    case "ui":
      return { pane: "general", focus: null };
    case "log":
      return { pane: "advanced", focus: null };
    case "text":
      return { pane: "advanced", focus: "text" };
    default:
      return null;
  }
}

/** Where the last pane used is kept, in this window's own storage. */
export const PANE_KEY = "bridgewatch.settings.pane";

/**
 * The pane to open on: the last one used, else Accounts.
 *
 * ⚠ Storage can be missing or refuse (a locked-down profile): that is never
 * an error, only a window that opens on Accounts.
 */
export function restorePane(storage: Pick<Storage, "getItem"> | null | undefined): Pane {
  try {
    const saved = paneFor(storage?.getItem(PANE_KEY));
    return saved?.pane ?? "accounts";
  } catch {
    return "accounts";
  }
}

export function rememberPane(storage: Pick<Storage, "setItem"> | null | undefined, pane: Pane): void {
  try {
    storage?.setItem(PANE_KEY, pane);
  } catch {
    // Nothing to do: the next window opens on Accounts.
  }
}

/** The registry tabs a pane renders, in order, each as one titled group. */
export const PANE_GROUPS: Record<"general" | "verdict", { tab: Tab; title: string }[]> = {
  general: [
    { tab: "icon", title: "Icon" },
    { tab: "ui", title: "App" },
  ],
  verdict: [{ tab: "verdict", title: "Verdict script" }],
};

// ---------------------------------------------------------------------------
// Groups inside one account and one watch
// ---------------------------------------------------------------------------

export type AccountGroup = "Sign-in" | "Server" | "Advanced";

/**
 * Which group an account key is shown in.
 *
 * Sign-in first: 2.0 adds an account by signing in, so the question the card
 * answers first is "how does this account get its token". The rarely changed
 * transport keys sit in a collapsed Advanced group.
 */
const ACCOUNT_GROUPS: Record<string, AccountGroup> = {
  token: "Sign-in",
  actions: "Sign-in",
  provider: "Server",
  base_url: "Server",
  api_path: "Advanced",
  header: "Advanced",
  timeout_secs: "Advanced",
  rate_limit_backoff: "Advanced",
};

export type WatchGroup =
  | "Source"
  | "Rows"
  | "Polling"
  | "Child pipelines"
  | "Deploy"
  | "Jobs"
  | "Notifications";

/** In the order they are shown. Only Source starts open. */
export const WATCH_GROUPS: WatchGroup[] = [
  "Source",
  "Rows",
  "Polling",
  "Child pipelines",
  "Deploy",
  "Jobs",
  "Notifications",
];

/**
 * Which group a watch key is shown in, by its config path.
 *
 * A key under a table (`show.*`, `poll.*`, `dive.*`, `notify.*`, `jobs.*`)
 * goes with its table; a top-level key is listed by name. ⛔ A key in neither
 * list is null, and `panes.test.ts` walks the registry so a new key fails the
 * suite with its path named rather than landing in a group nobody chose.
 */
const WATCH_TABLES: Record<string, WatchGroup> = {
  show: "Rows",
  poll: "Polling",
  dive: "Child pipelines",
  jobs: "Jobs",
  notify: "Notifications",
};

const WATCH_KEYS: Record<string, WatchGroup> = {
  id: "Source",
  account: "Source",
  project: "Source",
  ref: "Source",
  workflow: "Source",
  sources: "Source",
  role: "Source",
  mine: "Source",
  group: "Rows",
  fan_out_secs: "Rows",
  expect: "Rows",
  deploy_markers: "Deploy",
  post_deploy_failure: "Deploy",
  eta: "Deploy",
  sibling_failure: "Jobs",
};

/** The key after the wildcard: `accounts.*.rate_limit_backoff.max_secs` → `rate_limit_backoff.max_secs`. */
function rest(path: string, prefix: string): string[] {
  return path.startsWith(prefix) ? path.slice(prefix.length).split(".") : [];
}

export function accountGroup(path: string): AccountGroup | null {
  const [first] = rest(path, "accounts.*.");
  return (first && ACCOUNT_GROUPS[first]) || null;
}

export function watchGroup(path: string): WatchGroup | null {
  const keys = rest(path, "watches.*.");
  if (keys.length === 0) return null;
  if (keys.length > 1) return WATCH_TABLES[keys[0]] ?? null;
  return WATCH_KEYS[keys[0]] ?? null;
}
