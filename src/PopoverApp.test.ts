import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Snapshot, Status } from "./lib/types";

/**
 * The popover's own timer.
 *
 * ⛔ The shell hides this window the instant it loses focus, so "how often does
 * it poll" is not a question about the poller: it is a question about a
 * `setInterval` in a window nobody can see. The whole IPC surface is mocked
 * here, which is also the only way to mount this component at all — `inTauri()`
 * is false under jsdom and the real one returns early.
 */

const status: Status = {
  configPath: "/tmp/bridgewatch.toml",
  seeded: false,
  configOk: true,
  diagnostics: [],
  sinceLastPollSecs: 1,
  nextPollSecs: 9,
  launchAtLogin: false,
  platform: "macos",
  vibrancy: false,
};

const snapshot: Snapshot = {
  icon_state: "deployed",
  watches: [],
  errors: [],
  last_poll: "2026-09-17T15:00:00Z",
  request_log: [],
};

const calls = { status: 0, opened: 0 };

/** What the shell answers for the two "open" requests; reset per test. */
const shell = {
  status,
  pipelinesUrl: (): Promise<string | null> => Promise.resolve(null),
  openExternal: (_url: string): Promise<void> => Promise.resolve(),
  openWizard: (): Promise<void> => Promise.resolve(),
};

vi.mock("./lib/ipc", () => ({
  inTauri: () => true,
  getSnapshot: () => Promise.resolve(snapshot),
  getStatus: () => {
    calls.status++;
    return Promise.resolve(shell.status);
  },
  popoverOpened: () => {
    calls.opened++;
    return Promise.resolve(true);
  },
  onSnapshot: () => Promise.resolve(() => {}),
  hidePopover: () => Promise.resolve(),
  openExternal: (url: string) => shell.openExternal(url),
  openSettings: () => Promise.resolve(),
  openWizard: () => shell.openWizard(),
  pipelinesUrl: () => shell.pipelinesUrl(),
  quit: () => Promise.resolve(),
  refreshNow: () => Promise.resolve(true),
  resizePopover: () => Promise.resolve(),
}));

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;
let PopoverApp: typeof import("./PopoverApp.svelte").default;

// ⛔ Imported once, here, and not inside a test. A cold import compiles the whole
// popover tree, which took 8 to 35 s on a slow disk: inside the first test that
// was a 5 s timeout, and the timed-out test's interval then kept firing into the
// next one, so "catches up the moment it comes back" counted 3 calls, not 2.
beforeAll(async () => {
  ({ default: PopoverApp } = await import("./PopoverApp.svelte"));
}, 120_000);

beforeEach(() => {
  calls.status = 0;
  calls.opened = 0;
  shell.status = status;
  shell.pipelinesUrl = () => Promise.resolve(null);
  shell.openExternal = () => Promise.resolve();
  shell.openWizard = () => Promise.resolve();
  vi.useFakeTimers();
  // jsdom has no layout, so the resize measurement is a no-op; requestAnimationFrame
  // still has to exist for the click handler.
  vi.stubGlobal("requestAnimationFrame", (fn: FrameRequestCallback) => {
    fn(0);
    return 0;
  });
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function render() {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(PopoverApp, { target: host, props: {} });
  flushSync();
  return {
    seconds: (n: number) => {
      vi.advanceTimersByTime(n * 1000);
      flushSync();
    },
  };
}

describe("PopoverApp, while nobody is looking", () => {
  it("polls the shell every second while it has focus", async () => {
    const h = await render();
    const before = calls.status;
    h.seconds(3);
    expect(calls.status).toBe(before + 3);
  });

  it("stops polling once the window is blurred, which is when it is hidden", async () => {
    // ⛔ `WindowEvent::Focused(false)` hides the popover, so this window spends
    // nearly all of its life invisible — and it went on calling `get_status`
    // once a second, forever, to update a clock nobody was reading.
    const h = await render();
    window.dispatchEvent(new Event("blur"));
    const quiet = calls.status;
    h.seconds(5);
    expect(calls.status).toBe(quiet);
  });

  it("stops when the document is hidden even if no blur arrived", async () => {
    const h = await render();
    const spy = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    const quiet = calls.status;
    h.seconds(5);
    expect(calls.status).toBe(quiet);
    spy.mockRestore();
  });

  it("catches up the moment it comes back, rather than a second later", async () => {
    const h = await render();
    window.dispatchEvent(new Event("blur"));
    h.seconds(5);
    const quiet = calls.status;
    window.dispatchEvent(new Event("focus"));
    flushSync();
    // One immediate refresh for the stale clock, and the poll request the
    // shell expects on every open.
    expect(calls.status).toBe(quiet + 1);
    expect(calls.opened).toBe(1);
    h.seconds(2);
    expect(calls.status).toBe(quiet + 3);
  });

  it("stops for good once unmounted", async () => {
    const h = await render();
    void unmount(component!);
    component = null;
    const quiet = calls.status;
    h.seconds(5);
    expect(calls.status).toBe(quiet);
  });
});

describe("PopoverApp, a request the shell could not carry out", () => {
  async function settle() {
    for (let i = 0; i < 6; i++) await Promise.resolve();
    flushSync();
  }
  const button = (label: string) =>
    [...host.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === label)!;
  const notice = () => host.querySelector("[data-slot=popover-notice]")?.textContent?.trim() ?? "";

  it("says why Pipelines opened nothing", async () => {
    // The rejection used to be dropped, so the click did nothing at all.
    shell.pipelinesUrl = () => Promise.resolve("https://gitlab.com/g/p/-/pipelines");
    shell.openExternal = () => Promise.reject("the host is not a configured account's");
    await render();
    button("Pipelines").click();
    await settle();
    expect(notice()).toBe("Could not open the pipelines page: the host is not a configured account's");
  });

  it("says so when there is no pipelines page to open yet", async () => {
    await render();
    button("Pipelines").click();
    await settle();
    expect(notice()).toContain("No pipelines page");
  });

  it("says why the setup wizard did not open", async () => {
    shell.status = {
      ...status,
      configOk: false,
      firstRun: true,
      diagnostics: [{ severity: "error", path: "", message: "No configuration yet.", line: null, col: null }],
    };
    shell.openWizard = () => Promise.reject({ kind: "window", message: "the wizard window could not be created" });
    await render();
    await settle();
    button("Set up bridgewatch…").click();
    await settle();
    expect(notice()).toBe("Could not open the setup wizard: the wizard window could not be created");
  });

  it("forgets the notice once the popover is hidden", async () => {
    await render();
    button("Pipelines").click();
    await settle();
    window.dispatchEvent(new Event("blur"));
    flushSync();
    expect(notice()).toBe("");
  });
});
