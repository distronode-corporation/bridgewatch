import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import fixture from "./lib/__fixtures__/ca41ab28.json" with { type: "json" };
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

const calls = { status: 0, opened: 0, edits: [] as unknown[], resumes: 0, resizes: [] as number[], refreshes: 0 };

/** What the shell answers for the two "open" requests; reset per test. */
const shell = {
  status,
  pipelinesUrl: (): Promise<string | null> => Promise.resolve(null),
  openExternal: (_url: string): Promise<void> => Promise.resolve(),
  openWizard: (): Promise<void> => Promise.resolve(),
  applyConfigEdits: (_edits: unknown): Promise<{ ok: boolean; diagnostics: { message: string }[] }> =>
    Promise.resolve({ ok: true, diagnostics: [] }),
  resumeNotifications: (): Promise<void> => Promise.resolve(),
  resize: (_height: number): void => {},
  snapshot,
};

vi.mock("./lib/ipc", () => ({
  inTauri: () => true,
  applyConfigEdits: (edits: unknown) => {
    calls.edits.push(edits);
    return shell.applyConfigEdits(edits);
  },
  getSnapshot: () => Promise.resolve(shell.snapshot),
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
  refreshNow: () => {
    calls.refreshes++;
    return Promise.resolve(true);
  },
  resizePopover: (height: number) => {
    calls.resizes.push(height);
    shell.resize(height);
    return Promise.resolve();
  },
  resumeNotifications: () => {
    calls.resumes++;
    return shell.resumeNotifications();
  },
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
  calls.edits = [];
  calls.resumes = 0;
  calls.resizes = [];
  calls.refreshes = 0;
  shell.snapshot = snapshot;
  shell.resize = () => {};
  shell.status = status;
  shell.resumeNotifications = () => Promise.resolve();
  shell.applyConfigEdits = () => Promise.resolve({ ok: true, diagnostics: [] });
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
  // By accessible name: Pipelines is an icon button whose name is its aria-label.
  const button = (label: string) =>
    [...host.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => (b.getAttribute("aria-label") ?? b.textContent?.trim()) === label,
    )!;
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

describe("PopoverApp, the crash-report question", () => {
  async function settle() {
    for (let i = 0; i < 8; i++) await Promise.resolve();
    flushSync();
  }
  const question = () => host.querySelector("[data-slot=crash-reports-ask]");
  const answer = (which: "send" | "dont-send") =>
    host.querySelector<HTMLButtonElement>(`[data-slot=crash-reports-ask] [data-answer=${which}]`)!;
  const notice = () => host.querySelector("[data-slot=popover-notice]")?.textContent?.trim() ?? "";
  const asking: Status = { ...status, crashReports: "ask" };

  it("is put only while the shell says ask", async () => {
    for (const crashReports of ["unavailable", "off", "on", undefined] as const) {
      shell.status = { ...status, crashReports };
      await render();
      await settle();
      expect(question(), String(crashReports)).toBeNull();
      void unmount(component!);
      component = null;
      host.remove();
    }
    shell.status = asking;
    await render();
    await settle();
    expect(question()?.textContent).toContain("Send crash reports?");
  });

  it("offers Don't send first, as the default", async () => {
    shell.status = asking;
    await render();
    await settle();
    const labels = [...question()!.querySelectorAll("button")].map((b) => b.textContent?.trim());
    expect(labels).toEqual(["Don't send", "Send"]);
  });

  it.each([
    ["dont-send", false],
    ["send", true],
  ] as const)("records %s as ui.send_crash_reports = %s, once, and goes", async (which, value) => {
    shell.status = asking;
    // The shell answers the next status as answered, as a reload would.
    shell.applyConfigEdits = () => {
      shell.status = { ...status, crashReports: value ? "on" : "off" };
      return Promise.resolve({ ok: true, diagnostics: [] });
    };
    await render();
    await settle();
    // Two clicks before any re-render: one write.
    answer(which).click();
    answer(which).click();
    await settle();
    expect(calls.edits).toEqual([[{ op: "set", path: "ui.send_crash_reports", value: { boolean: value } }]]);
    expect(question()).toBeNull();
  });

  it("says why, and asks again, when the answer could not be saved", async () => {
    shell.status = asking;
    shell.applyConfigEdits = () =>
      Promise.resolve({ ok: false, diagnostics: [{ message: "permission denied" }] });
    await render();
    await settle();
    answer("send").click();
    await settle();
    expect(notice()).toBe("Could not save the crash-report choice: permission denied");
    expect(question()).not.toBeNull();
  });

  it("says why when the shell rejects the write outright", async () => {
    shell.status = asking;
    shell.applyConfigEdits = () => Promise.reject("the shell is gone");
    await render();
    await settle();
    answer("dont-send").click();
    await settle();
    expect(notice()).toBe("Could not save the crash-report choice: the shell is gone");
    expect(question()).not.toBeNull();
  });
});

describe("PopoverApp, paused notifications", () => {
  async function settle() {
    for (let i = 0; i < 8; i++) await Promise.resolve();
    flushSync();
  }
  const strip = () => host.querySelector("[data-slot=notifications-paused]");
  const resume = () => host.querySelector<HTMLButtonElement>("[data-slot=notifications-paused] [data-action=resume]")!;
  const notice = () => host.querySelector("[data-slot=popover-notice]")?.textContent?.trim() ?? "";

  it("shows nothing while notifications are on", async () => {
    for (const notificationsPaused of [undefined, null]) {
      shell.status = { ...status, notificationsPaused };
      await render();
      await settle();
      expect(strip(), String(notificationsPaused)).toBeNull();
      void unmount(component!);
      component = null;
      host.remove();
    }
  });

  it("says the pause lasts until resumed, and Resume ends it once however fast it is clicked", async () => {
    shell.status = { ...status, notificationsPaused: { until: null } };
    shell.resumeNotifications = () => {
      shell.status = { ...status, notificationsPaused: null };
      return Promise.resolve();
    };
    await render();
    await settle();
    expect(strip()?.textContent).toContain("Notifications paused until you resume");
    resume().click();
    resume().click();
    await settle();
    expect(calls.resumes).toBe(1);
    expect(strip()).toBeNull();
  });

  it("says when a timed pause runs out", async () => {
    shell.status = { ...status, notificationsPaused: { until: "2026-11-07T01:05:00Z" } };
    await render();
    await settle();
    expect(strip()?.getAttribute("data-until")).toBe("2026-11-07T01:05:00Z");
    expect(strip()?.textContent).toMatch(/Notifications paused until \S/);
    expect(strip()?.textContent).not.toContain("you resume");
  });

  it("says why when the shell refuses", async () => {
    shell.status = { ...status, notificationsPaused: { until: null } };
    shell.resumeNotifications = () => Promise.reject("the shell is gone");
    await render();
    await settle();
    resume().click();
    await settle();
    expect(notice()).toBe("Could not resume notifications: the shell is gone");
    expect(strip()).not.toBeNull();
  });
});

describe("PopoverApp, sizing the window to its content", () => {
  /**
   * A layout model, since jsdom lays nothing out. The window is `win` tall;
   * the header, notices and footer take `chrome`; the list fills the rest
   * (`flex-1`) and holds `content` of rows. As in a browser, the list's
   * `scrollHeight` is never less than its own box, and the shell clamps a
   * resize to between 80 and the configured maximum.
   */
  const MAX = 720;
  const HEADER = 40;
  const layout = { win: 400, chrome: 100, content: 500 };
  const restore: (() => void)[] = [];

  const list = () => host.querySelector<HTMLElement>("[data-scroll]")!;
  const pad = (el: HTMLElement) => parseFloat(el.style.paddingBottom) || 0;

  function stubLayout() {
    const proto = HTMLElement.prototype;
    const own = (name: "clientHeight" | "scrollHeight", get: (el: HTMLElement) => number) => {
      const before = Object.getOwnPropertyDescriptor(proto, name);
      Object.defineProperty(proto, name, {
        configurable: true,
        get(this: HTMLElement) {
          return this.hasAttribute("data-scroll") ? get(this) : 0;
        },
      });
      restore.push(() => {
        if (before) Object.defineProperty(proto, name, before);
        else delete (proto as unknown as Record<string, unknown>)[name];
      });
    };
    own("clientHeight", () => layout.win - layout.chrome);
    own("scrollHeight", (el) => Math.max(layout.content + pad(el), layout.win - layout.chrome));
    const rect = vi.spyOn(proto, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.hasAttribute("data-popover")) return new DOMRect(0, 0, 300, layout.win);
      const parent = this.parentElement;
      if (parent?.hasAttribute("data-scroll")) {
        // As a browser does, a row that is not drawn reports a zero rect at the origin.
        if (this.style.display === "none") return new DOMRect(0, 0, 0, 0);
        const rows = [...parent.children].filter((row) => (row as HTMLElement).style.display !== "none");
        const share = layout.content / rows.length;
        return new DOMRect(0, HEADER + rows.indexOf(this) * share, 300, share);
      }
      return new DOMRect(0, 0, 0, 0);
    });
    restore.push(() => rect.mockRestore());
    shell.resize = (h) => (layout.win = Math.min(Math.max(h, 80), MAX));
  }

  beforeEach(() => {
    Object.assign(layout, { win: 400, chrome: 100, content: 500 });
    stubLayout();
  });
  afterEach(() => {
    while (restore.length) restore.pop()!();
  });

  async function grown() {
    const h = await render();
    list().style.paddingBottom = "12px";
    h.seconds(1);
    // Grows to the header and footer plus every row and the list's padding.
    expect(calls.resizes.at(-1)).toBe(100 + 500 + 12);
    expect(layout.win).toBe(612);
    return h;
  }

  it("shrinks the window again once the content gets smaller", async () => {
    const h = await grown();
    // A watch collapsed: the rows now take 150, but the list's box is still
    // the 512 the old window gave it, so its scrollHeight still says 512.
    layout.content = 150;
    h.seconds(1);
    expect(calls.resizes.at(-1)).toBe(100 + 150 + 12);
    expect(layout.win).toBe(262);
  });

  it("comes back down from the cap", async () => {
    const h = await grown();
    layout.content = 2000;
    h.seconds(1);
    expect(layout.win).toBe(MAX);
    layout.content = 150;
    h.seconds(1);
    expect(layout.win).toBe(262);
  });

  it("adds the outer rows' margins and ignores a row that is not drawn", async () => {
    const h = await grown();
    const hidden = document.createElement("div");
    hidden.style.display = "none";
    const last = document.createElement("div");
    last.style.marginBottom = "4px";
    list().append(hidden, last);
    (list().firstElementChild as HTMLElement).style.marginTop = "8px";
    h.seconds(1);
    // The hidden row's zero rect at the origin would otherwise add the 40 above the list.
    expect(calls.resizes.at(-1)).toBe(100 + 8 + 500 + 4 + 12);
  });

  it("is the chrome and the list's padding when nothing in the list is drawn", async () => {
    const h = await grown();
    (list().firstElementChild as HTMLElement).style.display = "none";
    h.seconds(1);
    expect(calls.resizes.at(-1)).toBe(100 + 12);
  });

  it("asks once per real change, never again because of its own resize", async () => {
    const h = await grown();
    layout.content = 150;
    h.seconds(1);
    const settled = calls.resizes.length;
    h.seconds(5);
    document.body.click();
    flushSync();
    expect(calls.resizes.length).toBe(settled);
    // Sub-pixel layout noise is not a change either.
    layout.content = 150.6;
    h.seconds(2);
    expect(calls.resizes.length).toBe(settled);
  });
});

describe("PopoverApp, the keyboard", () => {
  async function settle() {
    for (let i = 0; i < 8; i++) await Promise.resolve();
    flushSync();
  }
  const press = (key: string, init: KeyboardEventInit = {}) => {
    const event = new KeyboardEvent("keydown", { key, cancelable: true, ...init });
    window.dispatchEvent(event);
    flushSync();
    return event;
  };
  const shas = () => [...host.querySelectorAll<HTMLElement>("[data-scroll] .row")].map((r) => r.querySelector(".sha"));

  it("moves between rows with the arrows and j and k, landing on the commit link", async () => {
    // The recorded snapshot's primary watch, with a second, older row under it.
    const recorded = fixture as unknown as Snapshot;
    const watch = recorded.watches[0];
    const older = { ...watch.rows[0], id: watch.rows[0].id - 1, sha7: "0000000" };
    shell.snapshot = { ...recorded, watches: [{ ...watch, rows: [watch.rows[0], older] }] };
    await render();
    await settle();
    const rows = shas();
    expect(rows.length).toBe(2);
    expect(press("ArrowDown").defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(rows[0]);
    press("j");
    expect(document.activeElement).toBe(rows[1]);
    press("k");
    expect(document.activeElement).toBe(rows[0]);
    press("End");
    expect(document.activeElement).toBe(rows[rows.length - 1]);
  });

  it("refreshes on r, and leaves a key with a modifier to the system", async () => {
    await render();
    await settle();
    press("r");
    expect(calls.refreshes).toBe(1);
    expect(press("r", { ctrlKey: true }).defaultPrevented).toBe(false);
    expect(calls.refreshes).toBe(1);
  });
});
