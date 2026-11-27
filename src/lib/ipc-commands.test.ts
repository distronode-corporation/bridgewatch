// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import mainRs from "../../src-tauri/src/main.rs?raw";
import ipcSource from "./ipc.ts?raw";

/**
 * The command surface in ipc.ts, held to the shell's.
 *
 * Every wrapper is one `invoke` with a fixed name and argument shape; a
 * renamed command or argument on either side would fail only at runtime, in
 * the webview. These tests pin each wrapper's name and arguments, and check
 * every name ipc.ts sends against the `generate_handler!` list in the shell's
 * main.rs, so a command the shell does not register cannot be sent.
 */

const invoked: { cmd: string; args: Record<string, unknown> | undefined }[] = [];
const listeners = new Map<string, (event: { payload: unknown }) => void>();
let rejectNext: unknown = undefined;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => {
    invoked.push({ cmd, args });
    if (rejectNext !== undefined) {
      const error = rejectNext;
      rejectNext = undefined;
      return Promise.reject(error);
    }
    return Promise.resolve(null);
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  },
}));

beforeEach(() => {
  invoked.length = 0;
  listeners.clear();
  rejectNext = undefined;
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  // jsdom has no navigator.clipboard; the copyText tests give it one.
  delete (navigator as unknown as Record<string, unknown>).clipboard;
  vi.restoreAllMocks();
});

/** Give jsdom's navigator the clipboard both webviews have. */
function stubClipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
}

/** The command names the shell registers, read from its `generate_handler!` call. */
function registeredCommands(): Set<string> {
  const start = mainRs.indexOf("generate_handler![");
  expect(start).toBeGreaterThan(-1);
  const list = mainRs.slice(start, mainRs.indexOf("]", start));
  return new Set([...list.matchAll(/\b\w+::(\w+)/g)].map((m) => m[1]));
}

describe("the plain command wrappers", () => {
  it("send each command by its shell name, with its arguments", async () => {
    const ipc = await import("./ipc");
    await ipc.getSnapshot();
    await ipc.getStatus();
    await ipc.refreshNow();
    await ipc.popoverOpened();
    await ipc.resumeNotifications();
    await ipc.acknowledgeFailure("main-push", true);
    await ipc.readConfigText();
    await ipc.getConfigJson();
    await ipc.moveWatch("main-push", -1);
    await ipc.readThemeCss();
    await ipc.validateConfigText("x = 1");
    await ipc.saveConfigText("x = 1", "x = 0");
    await ipc.saveConfigText("x = 1", "x = 0", "q1");
    await ipc.applyConfigEdits([]);
    await ipc.applyConfigEdits([], "q2");
    await ipc.reloadConfig();
    await ipc.openConfigFile();
    await ipc.pipelinesUrl();
    await ipc.openSettings();
    await ipc.openSettings("text");
    await ipc.takeSettingsTab();
    await ipc.hidePopover();
    await ipc.resizePopover(320);
    await ipc.quit();
    await ipc.setOwnToken("gitlab.com", "glpat-test");
    await ipc.clearOwnToken("gitlab.com");
    await ipc.ownTokenStatus("gitlab.com");
    await ipc.jobLogTail("main-push", "https://gitlab.com/x/-/jobs/1");
    await ipc.jobAction("main-push", "https://gitlab.com/x/-/jobs/1", "retry");
    await ipc.openWizard();
    expect(invoked).toEqual([
      { cmd: "get_snapshot", args: undefined },
      { cmd: "get_status", args: undefined },
      { cmd: "refresh_now", args: undefined },
      { cmd: "popover_opened", args: undefined },
      { cmd: "resume_notifications", args: undefined },
      { cmd: "acknowledge_failure", args: { watch: "main-push", acknowledged: true } },
      { cmd: "read_config_text", args: undefined },
      { cmd: "get_config_json", args: undefined },
      { cmd: "move_watch", args: { id: "main-push", delta: -1 } },
      { cmd: "read_theme_css", args: undefined },
      { cmd: "validate_config_text", args: { text: "x = 1" } },
      // A missing confirmation goes as null: the shell's `Option` reads null as None.
      { cmd: "save_config_text", args: { text: "x = 1", base: "x = 0", confirm: null } },
      { cmd: "save_config_text", args: { text: "x = 1", base: "x = 0", confirm: "q1" } },
      { cmd: "apply_config_edits", args: { edits: [], confirm: null } },
      { cmd: "apply_config_edits", args: { edits: [], confirm: "q2" } },
      { cmd: "reload_config", args: undefined },
      { cmd: "open_config_file", args: undefined },
      { cmd: "pipelines_url", args: undefined },
      { cmd: "open_settings", args: { tab: null } },
      { cmd: "open_settings", args: { tab: "text" } },
      { cmd: "take_settings_tab", args: undefined },
      { cmd: "hide_popover", args: undefined },
      { cmd: "resize_popover", args: { height: 320 } },
      { cmd: "quit", args: undefined },
      { cmd: "set_own_token", args: { account: "gitlab.com", token: "glpat-test" } },
      { cmd: "clear_own_token", args: { account: "gitlab.com" } },
      { cmd: "own_token_status", args: { account: "gitlab.com" } },
      { cmd: "job_log_tail", args: { watch: "main-push", url: "https://gitlab.com/x/-/jobs/1" } },
      { cmd: "job_action", args: { watch: "main-push", url: "https://gitlab.com/x/-/jobs/1", action: "retry" } },
      { cmd: "open_wizard", args: undefined },
    ]);
  });

  it("sends only commands the shell registers", async () => {
    const ipc = await import("./ipc");
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const sent = new Set([...ipcSource.matchAll(/invoke<[^>]*(?:<[^>]*>[^>]*)?>\(\s*"(\w+)"/g)].map((m) => m[1]));
    // A sanity bound: the regex found the surface, not a handful of stray matches.
    expect(sent.size).toBeGreaterThan(40);
    const registered = registeredCommands();
    expect([...sent].filter((cmd) => !registered.has(cmd))).toEqual([]);
    // And the commands this test file drives are among them.
    await ipc.openExternal("https://gitlab.com");
    expect(invoked.every((i) => registered.has(i.cmd))).toBe(true);
  });
});

describe("the event subscriptions", () => {
  it("hand each handler the event's payload", async () => {
    const ipc = await import("./ipc");
    const snapshots: unknown[] = [];
    const tabs: string[] = [];
    let changed = 0;
    const progress: unknown[] = [];
    await ipc.onSnapshot((s) => snapshots.push(s));
    await ipc.onOpenSettings((tab) => tabs.push(tab));
    const unlisten = await ipc.onConfigChanged(() => changed++);
    await ipc.oauthApi().onProgress!((p) => progress.push(p));
    expect([...listeners.keys()].sort()).toEqual(["config-changed", "oauth-progress", "open-settings", "snapshot"]);
    listeners.get("snapshot")!({ payload: { watches: [] } });
    listeners.get("open-settings")!({ payload: "text" });
    listeners.get("config-changed")!({ payload: null });
    listeners.get("oauth-progress")!({ payload: { id: "f1", state: "waiting" } });
    expect(snapshots).toEqual([{ watches: [] }]);
    expect(tabs).toEqual(["text"]);
    expect(changed).toBe(1);
    expect(progress).toEqual([{ id: "f1", state: "waiting" }]);
    unlisten();
    expect(listeners.has("config-changed")).toBe(false);
  });
});

describe("openExternal", () => {
  it("sends nothing outside the Tauri webview", async () => {
    const { openExternal, inTauri } = await import("./ipc");
    expect(inTauri()).toBe(false);
    await openExternal("https://gitlab.com/x/-/jobs/1");
    expect(invoked).toEqual([]);
  });

  it("logs a refusal from the shell and does not throw", async () => {
    const { openExternal } = await import("./ipc");
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    rejectNext = "host not configured";
    await expect(openExternal("https://evil.example/")).resolves.toBeUndefined();
    expect(invoked).toEqual([{ cmd: "open_link", args: { url: "https://evil.example/" } }]);
    expect(error).toHaveBeenCalledWith("not opened", "https://evil.example/", "host not configured");
  });
});

describe("copyText", () => {
  it("writes to the clipboard and says so", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    stubClipboard(writeText);
    const { copyText } = await import("./ipc");
    await expect(copyText("WDJB-MJHT")).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith("WDJB-MJHT");
  });

  it("answers false when the clipboard refuses, as an unfocused document makes it", async () => {
    const refused = new DOMException("Document is not focused.", "NotAllowedError");
    stubClipboard(() => Promise.reject(refused));
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { copyText } = await import("./ipc");
    await expect(copyText("WDJB-MJHT")).resolves.toBe(false);
    expect(error).toHaveBeenCalledWith("clipboard write failed", refused);
  });
});

describe("toDiagnosticView", () => {
  it("keeps a shell diagnostic's resolved line and column", async () => {
    const { toDiagnosticView } = await import("./ipc");
    expect(toDiagnosticView({ severity: "error", path: "accounts", message: "missing", line: 3, col: 7 })).toEqual({
      severity: "error",
      path: "accounts",
      message: "missing",
      line: 3,
      col: 7,
    });
  });

  it("reads anything else as an error, with what it can say about it", async () => {
    const { toDiagnosticView } = await import("./ipc");
    expect(toDiagnosticView(null)).toEqual({ severity: "error", path: "", message: "null", line: null, col: null });
    expect(toDiagnosticView("boom")).toEqual({ severity: "error", path: "", message: "boom", line: null, col: null });
    expect(toDiagnosticView({ severity: "fatal", path: 4, message: { x: 1 }, line: "3", col: "7" })).toEqual({
      severity: "error",
      path: "",
      message: "[object Object]",
      line: null,
      col: null,
    });
  });

  it("gives a save with no diagnostic list an empty one", async () => {
    const core = await import("@tauri-apps/api/core");
    vi.spyOn(core, "invoke").mockResolvedValueOnce({ path: "/x.toml", diagnostics: "not a list" });
    const { wizardApi } = await import("./ipc");
    const saved = await wizardApi().save({} as never, {});
    expect(saved.diagnostics).toEqual([]);
  });
});

describe("wizardApi().suggestExpectedWorkflows", () => {
  it("asks the shell with the connection, the window's account filling the gap", async () => {
    const { wizardApi } = await import("./ipc");
    const connection = { provider: "github" as const, base_url: "https://api.github.com", token: { own: true } };
    await wizardApi("hub").suggestExpectedWorkflows!(connection, "acme/web", "main", ["push"]);
    expect(invoked).toEqual([
      {
        cmd: "wizard_suggest_expected_workflows",
        args: { connection: { ...connection, account: "hub" }, project: "acme/web", refName: "main", sources: ["push"] },
      },
    ]);
  });
});

describe("wizardInitial on a hostile or partial configuration", () => {
  it("reads an account whose entry is null as an account with nothing to pre-fill", async () => {
    const { wizardInitial } = await import("./ipc");
    expect(wizardInitial({ accounts: { lab: null }, watches: "nope" })).toEqual({ account: "lab" });
  });

  it("takes a watch with no role as primary", async () => {
    const { wizardInitial } = await import("./ipc");
    expect(wizardInitial({ accounts: { lab: {} }, watches: [{ account: "lab", id: "w" }] })).toEqual({
      account: "lab",
      watch_id: "w",
    });
  });

  it("pre-fills no field from a watch whose fields have the wrong types", async () => {
    const { wizardInitial } = await import("./ipc");
    const watch = { account: "lab", id: 7, project: null, ref: ["main"], sources: "push", deploy_markers: "x" };
    expect(wizardInitial({ accounts: { lab: { base_url: 1, token: "glpat" } }, watches: [watch] })).toEqual({
      account: "lab",
    });
  });

  it("falls back to the account's first watch when none of its watches is primary", async () => {
    const { wizardInitial } = await import("./ipc");
    expect(
      wizardInitial({
        accounts: { lab: {} },
        watches: [
          { account: "other", id: "o", role: "primary" },
          { account: "lab", id: "nightly", role: "secondary", project: { id: 1 } },
        ],
      }),
    ).toEqual({ account: "lab", watch_id: "nightly" });
  });
});
