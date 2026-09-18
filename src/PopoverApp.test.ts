import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

vi.mock("./lib/ipc", () => ({
  inTauri: () => true,
  getSnapshot: () => Promise.resolve(snapshot),
  getStatus: () => {
    calls.status++;
    return Promise.resolve(status);
  },
  popoverOpened: () => {
    calls.opened++;
    return Promise.resolve(true);
  },
  onSnapshot: () => Promise.resolve(() => {}),
  hidePopover: () => Promise.resolve(),
  openExternal: () => Promise.resolve(),
  openSettings: () => Promise.resolve(),
  pipelinesUrl: () => Promise.resolve(null),
  quit: () => Promise.resolve(),
  refreshNow: () => Promise.resolve(true),
  resizePopover: () => Promise.resolve(),
}));

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  calls.status = 0;
  calls.opened = 0;
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
  const { default: PopoverApp } = await import("./PopoverApp.svelte");
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
