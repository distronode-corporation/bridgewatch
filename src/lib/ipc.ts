/**
 * The frontend's whole view of the shell.
 *
 * Every `invoke` in the application goes through this file, so the command
 * surface is one list rather than a grep. Nothing here decides anything: it
 * calls a command and returns what came back.
 *
 * ⚠ Nothing in this module may run at import time. The component tests import
 * the same modules under jsdom, where there is no Tauri IPC at all, and a
 * top-level `invoke` would turn every test into an unhandled rejection.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  Confirmable,
  Connection,
  CliTokenDetection,
  Identity,
  MarkerSuggestions,
  ProjectListing,
  ResolvedProject,
  TokenSource,
  WizardAnswers,
  WizardApi,
  WizardPreview,
  WizardSaveResult,
} from "../components/wizard/api";
import type {
  OAuthApi,
  OAuthAvailability,
  SignInProgress,
  SignInStatus,
  SignedIn,
  StartedSignIn,
} from "./oauth";
import type { DiagnosticView, Edit, Snapshot, Status, Validation } from "./types";

/** True inside the Tauri webview, false under vitest and `vite dev` in a browser. */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export const getSnapshot = () => invoke<Snapshot>("get_snapshot");
export const getStatus = () => invoke<Status>("get_status");
export const refreshNow = () => invoke<boolean>("refresh_now");
export const popoverOpened = () => invoke<boolean>("popover_opened");
export const readConfigText = () => invoke<string>("read_config_text");
export const getConfigJson = () =>
  invoke<{ config: unknown; job_order: string[][] } | null>("get_config_json");
export const moveWatch = (id: string, delta: number) =>
  invoke<Validation>("move_watch", { id, delta });
export const readThemeCss = () => invoke<string | null>("read_theme_css");
export const validateConfigText = (text: string) =>
  invoke<Validation>("validate_config_text", { text });
/**
 * Write the text tab's buffer. `base` is the file text the buffer was seeded
 * from: the shell refuses (`conflict: true`) when the file no longer holds it.
 * `confirm` is the id of a question the shell asked about exactly this text.
 */
export const saveConfigText = (text: string, base: string, confirm?: string) =>
  invoke<Validation>("save_config_text", { text, base, confirm: confirm ?? null });
export const applyConfigEdits = (edits: Edit[], confirm?: string) =>
  invoke<Validation>("apply_config_edits", { edits, confirm: confirm ?? null });
export const reloadConfig = () => invoke<void>("reload_config");
export const openConfigFile = () => invoke<void>("open_config_file");
export const pipelinesUrl = () => invoke<string | null>("pipelines_url");
export const openSettings = (tab?: string) => invoke<void>("open_settings", { tab: tab ?? null });
export const takeSettingsTab = () => invoke<string | null>("take_settings_tab");
export const hidePopover = () => invoke<void>("hide_popover");
export const resizePopover = (height: number) => invoke<void>("resize_popover", { height });
export const quit = () => invoke<void>("quit");
export const setOwnToken = (account: string, token: string) =>
  invoke<void>("set_own_token", { account, token });
export const clearOwnToken = (account: string) => invoke<void>("clear_own_token", { account });
export const getLaunchAtLogin = () => invoke<boolean>("get_launch_at_login");
export const setLaunchAtLogin = (enabled: boolean) =>
  invoke<boolean>("set_launch_at_login", { enabled });

/** Subscribe to the poller's per-tick snapshot. */
export function onSnapshot(handler: (s: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>("snapshot", (event) => handler(event.payload));
}

/** Subscribe to the tray menu asking Settings to change tab. */
export function onOpenSettings(handler: (tab: string) => void): Promise<UnlistenFn> {
  return listen<string>("open-settings", (event) => handler(event.payload));
}

/** Subscribe to "the config file changed", from any writer. */
export function onConfigChanged(handler: () => void): Promise<UnlistenFn> {
  return listen<null>("config-changed", () => handler());
}

/**
 * Open a URL in the user's browser.
 *
 * ⛔ Never navigate the webview. The popover is a 440px undecorated window with
 * no back button and no address bar: a GitLab page loaded into it is a trap the
 * only escape from which is quitting the app.
 *
 * ⛔ And never the opener plugin from here: the shell's `open_link` is the one
 * door, and it opens only URLs on a configured account's scheme, host and port
 * (a GitLab instance chooses every `web_url` this window renders). The webview
 * holds no opener permission at all.
 */
export async function openExternal(url: string | null | undefined): Promise<void> {
  if (!url) return;
  if (!inTauri()) return;
  try {
    await invoke<void>("open_link", { url });
  } catch (error) {
    console.error("not opened", url, error);
  }
}

/**
 * Put text on the clipboard.
 *
 * `navigator.clipboard` rather than the clipboard-manager plugin: it is
 * available in both webviews and costs no extra dependency, no extra
 * capability and no extra permission. It needs the document to be focused,
 * which for a button inside the popover it is.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (error) {
    console.error("clipboard write failed", error);
    return false;
  }
}

// ---------------------------------------------------------------------------
// The first-run wizard
// ---------------------------------------------------------------------------

/**
 * Show the wizard window. Settings' "Setup wizard…" entry and the popover's
 * first-run strip call this; the shell also shows it itself on a first launch
 * with no config file, and from the tray.
 *
 * There is no close command: `wizard_save` hides the window once the file is
 * written, and `wizard_skip` hides it and opens Settings on the text tab.
 */
export const openWizard = () => invoke<void>("open_wizard");

/**
 * A diagnostic as the wizard renders it.
 *
 * The shell's `Validation` already carries `DiagnosticView` (line/col
 * resolved); the core's own `Diagnostic` carries a byte `span` instead. Either
 * is accepted, so the wizard never sees the difference.
 */
export function toDiagnosticView(d: unknown): DiagnosticView {
  const o = (d ?? {}) as Record<string, unknown>;
  return {
    severity: o.severity === "warning" ? "warning" : "error",
    path: typeof o.path === "string" ? o.path : "",
    message: typeof o.message === "string" ? o.message : String(d),
    line: typeof o.line === "number" ? o.line : null,
    col: typeof o.col === "number" ? o.col : null,
  };
}

function diagnosticsIn(list: unknown): DiagnosticView[] {
  return Array.isArray(list) ? list.map(toDiagnosticView) : [];
}

/**
 * The wizard's `WizardApi`, backed by the shell's commands.
 *
 * Nothing here decides anything: every method is one command. Rejections pass
 * through untouched, because the shell rejects with the core's
 * `WizardFailure` (`{kind, message, issues, diagnostics}`) and the wizard reads
 * `issues` off it to send the user to the step at fault.
 *
 * ⛔ The pasted token goes out ONLY as `secret` (on a `Connection`, or as
 * `save`'s option). It is never copied into the answers.
 */
export function wizardApi(fallbackAccount?: string): WizardApi {
  /**
   * The shell's `Connection.account` names whose stored token an `{own: true}`
   * source reads; without it an `own` source finds nothing. The wizard sends
   * it when it knows it; the account the window was opened for fills the gap.
   */
  const withAccount = (connection: Connection): Connection & { account?: string } => {
    const given = (connection as Connection & { account?: string }).account;
    if (given || !fallbackAccount) return connection;
    return { ...connection, account: fallbackAccount };
  };
  return {
    detectCliToken: (baseUrl, provider) =>
      invoke<CliTokenDetection>("wizard_detect_cli_token", { baseUrl, provider }),
    testConnection: (connection) =>
      invoke<Confirmable<Identity>>("wizard_test_connection", { connection: withAccount(connection) }),
    listProjects: (connection, search) =>
      invoke<Confirmable<ProjectListing>>("wizard_list_projects", {
        connection: withAccount(connection),
        search: search ?? null,
      }),
    resolveProject: (connection, idOrPath) =>
      invoke<Confirmable<ResolvedProject>>("wizard_resolve_project", {
        connection: withAccount(connection),
        idOrPath,
      }),
    suggestDeployMarkers: (connection, project, refName, workflow) =>
      invoke<Confirmable<MarkerSuggestions>>("wizard_suggest_deploy_markers", {
        connection: withAccount(connection),
        project,
        refName,
        // Only when there is one: the shell reads a missing key as `None`, so
        // a GitLab call carries exactly the three keys it always did.
        ...(workflow ? { workflow } : {}),
      }),
    previewConfig: async (answers) => {
      const preview = await invoke<WizardPreview>("wizard_preview_config", { answers });
      return { ...preview, warnings: diagnosticsIn(preview.warnings) };
    },
    save: async (answers, options) => {
      const result = await invoke<WizardSaveResult>("wizard_save", {
        answers,
        secret: options.secret ?? null,
        confirm: options.confirm ?? null,
        // Only when the account step signed in: the shell moves that sign-in
        // to the saved account name. Absent, every other save is unchanged.
        ...(options.oauthAccount ? { oauthAccount: options.oauthAccount } : {}),
      });
      return { ...result, diagnostics: diagnosticsIn(result.diagnostics) };
    },
    skip: () => invoke<void>("wizard_skip"),
  };
}

/**
 * What the wizard pre-fills on a re-run, read off the current configuration.
 *
 * The first account and the first primary watch (else the first watch): the
 * wizard edits the file in place, so these are the entries it would touch.
 * Only non-secret fields; a token SOURCE is not a token.
 */
export function wizardInitial(config: unknown): Partial<WizardAnswers> | undefined {
  const c = (config ?? {}) as Record<string, unknown>;
  const accounts = (c.accounts ?? {}) as Record<string, Record<string, unknown>>;
  const watches = (Array.isArray(c.watches) ? c.watches : []) as Record<string, unknown>[];
  const account = Object.keys(accounts)[0];
  if (account === undefined) return undefined;
  const acc = accounts[account] ?? {};
  const watch =
    watches.find((w) => w.account === account && (w.role ?? "primary") === "primary") ??
    watches.find((w) => w.account === account);
  const initial: Partial<WizardAnswers> = { account };
  // Only a GitHub account says so: absent is GitLab, and a GitLab re-run
  // pre-fills exactly what it always did.
  if (acc.provider === "github") initial.provider = "github";
  if (typeof acc.base_url === "string") initial.base_url = acc.base_url;
  if (acc.token && typeof acc.token === "object") initial.token = acc.token as TokenSource;
  if (watch) {
    if (typeof watch.project === "number" || typeof watch.project === "string") {
      initial.project = watch.project;
    }
    if (typeof watch.id === "string") initial.watch_id = watch.id;
    if (typeof watch.ref === "string") initial.ref_name = watch.ref;
    if (Array.isArray(watch.sources)) initial.sources = watch.sources.map(String);
    if (initial.provider === "github" && typeof watch.workflow === "string") initial.workflow = watch.workflow;
    if (Array.isArray(watch.deploy_markers)) {
      initial.deploy_markers = watch.deploy_markers.map(String);
    }
  }
  const ui = (c.ui ?? {}) as Record<string, unknown>;
  if (typeof ui.launch_at_login === "boolean") initial.launch_at_login = ui.launch_at_login;
  return initial;
}

// ---------------------------------------------------------------------------
// Sign in with GitHub / GitLab
// ---------------------------------------------------------------------------

/**
 * The sign-in commands, as the wizard and Settings use them.
 *
 * ⛔ No token crosses here in either direction. The verification page is
 * opened by the SHELL (`oauth_open_verification`), which trusts the host of
 * the account being signed in for that one open, never by the webview.
 */
export function oauthApi(): OAuthApi {
  return {
    availability: (provider, baseUrl, clientId) =>
      invoke<OAuthAvailability>("oauth_availability", { provider, baseUrl, clientId: clientId ?? null }),
    start: (request) => invoke<StartedSignIn>("oauth_start", { request }),
    wait: (id) => invoke<SignedIn>("oauth_wait", { id }),
    cancel: (id) => invoke<void>("oauth_cancel", { id }),
    openVerification: (id) => invoke<void>("oauth_open_verification", { id }),
    openInstall: (provider, baseUrl) => invoke<void>("oauth_open_install", { provider, baseUrl }),
    status: (account) => invoke<SignInStatus | null>("oauth_status", { account }),
    signOut: (account) => invoke<void>("oauth_sign_out", { account }),
    copy: copyText,
    onProgress: (handler) => listen<SignInProgress>("oauth-progress", (event) => handler(event.payload)),
  };
}
