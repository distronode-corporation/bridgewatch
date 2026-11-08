import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import fixture from "../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { Snapshot, Status } from "../lib/types";
import Popover from "./Popover.svelte";

/**
 * The rendering proof.
 *
 * ⛔ The GUI itself is never launched here — a tray icon and two windows on
 * somebody's desktop is not a test result, and resolving a token would raise a
 * Keychain prompt with nobody to answer it. What this does instead is mount the
 * popover against a REAL snapshot: `ca41ab28.json` is the output of
 *
 *     bridgewatch check --fixture tests/fixtures/ca41ab28-deployed-with-failure --json
 *
 * against a recorded copy of gitlab.com pipeline 2857464986, the one that
 * deployed the website while the android bridge failed. If the rendering path
 * is broken, this fails; if the verdict engine changes shape, this fails with
 * the field named.
 */
const snapshot = fixture as unknown as Snapshot;

describe("Popover, rendered from a recorded pipeline", () => {
  let host: HTMLDivElement;
  let component: Record<string, unknown>;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.append(host);
    component = mount(Popover, {
      target: host,
      props: { snapshot, now: Date.parse("2026-09-17T15:00:00Z") },
    });
  });

  afterEach(() => {
    void unmount(component);
    host.remove();
  });

  it("is the fixture everybody means by ca41ab28", () => {
    // Guards the assertions below against a fixture swap: every one of them
    // would still pass against some other pipeline, and mean nothing.
    expect(snapshot.icon_state).toBe("deployed_with_failure");
    expect(snapshot.watches[0].rows[0].sha7).toBe("ca41ab2");
  });

  it("leads with the tray state as a phrase, not an identifier", () => {
    const headline = host.querySelector("[data-icon-state]");
    expect(headline?.getAttribute("data-icon-state")).toBe("deployed_with_failure");
    expect(headline?.textContent?.trim()).toBe("deployed with failure");
  });

  it("puts the primary watch first with its own headline", () => {
    const sections = [...host.querySelectorAll("[data-watch]")];
    expect(sections.map((s) => s.getAttribute("data-watch"))).toEqual([
      "main-push",
      "hourly",
      "preflights",
    ]);
    expect(sections[0].getAttribute("data-role")).toBe("primary");
    expect(sections[0].querySelector("[data-headline]")?.getAttribute("data-headline")).toBe(
      "deployed_with_failure",
    );
  });

  it("shows the pipeline row on two lines, with the verdict said once", () => {
    // ⚠ Asserted against the ROW's own cells, not against the popover's whole
    // text. Every one of these substrings also occurs in the watch id
    // (`main-push`) and the tray headline, so the text-wide version of this
    // test passed unchanged against `rows: []` — a watch rendering nothing at
    // all. The empty case is now its own test below.
    const rows = host.querySelectorAll('[data-watch="main-push"] .row');
    expect(rows.length).toBe(1);
    // Line 1 is the ref; line 2 is what the pipeline is.
    const head = rows[0].querySelector(".head");
    const sub = rows[0].querySelector(".sub");
    expect(head?.querySelector(".ref")?.textContent?.trim()).toBe("main");
    expect(sub?.querySelector(".sha")?.textContent?.trim()).toBe("ca41ab2");
    expect(sub?.querySelector(".source")?.textContent?.trim()).toBe("push");
    // The deploy marker is labelled, not a lone job name that reads as wrapped text.
    expect(sub?.querySelector(".marker")?.textContent?.replace(/\s+/g, " ").trim()).toBe("deploy job deploy:origins");
    // ⛔ Said once. `deploy: "live"` on a "deployed with failure" row added
    // nothing, and the README shot read "deployed deployed": no pill. The
    // watch header right above already says the state, so the row says it to
    // screen readers only.
    expect(head?.querySelector(".deploy")).toBe(null);
    const state = head?.querySelector(".state");
    expect(state?.textContent?.trim()).toBe("deployed with failure");
    expect(state?.classList.contains("sr-only")).toBe(true);
    expect(rows[0].closest("[data-watch]")?.querySelector("[data-headline]")?.textContent?.trim()).toBe(
      "deployed with failure",
    );
  });

  it("links a sibling failure to the trigger job, not to nothing", () => {
    // `sibling_failures` names `trigger:android`, which is a BRIDGE: it is not
    // in `parent_jobs`, so looking there alone rendered the one name on the row
    // that explains the amber icon as unclickable text.
    const note = [...host.querySelectorAll(".notes .note")].find((n) =>
      n.textContent?.includes("sibling"),
    );
    expect(note, "no sibling note on the row").toBeTruthy();
    const link = note!.querySelector("a");
    expect(link?.textContent?.trim()).toBe("trigger:android");
    expect(link?.getAttribute("href")).toBe(
      snapshot.watches[0].rows[0].bridges.find((b) => b.name === "trigger:android")?.web_url,
    );
  });

  it("renders every bridge as a sub-row", () => {
    // Four in this pipeline. The count is asserted so a bridge silently
    // disappearing fails here rather than in somebody's menu bar.
    const main = host.querySelector('[data-watch="main-push"]')!;
    const names = [...main.querySelectorAll("[data-bridge]")].map((n) => n.getAttribute("data-bridge"));
    expect(names).toEqual(["trigger:ios", "trigger:android", "trigger:voice_agent", "trigger:website"]);
    expect(names.length).toBe(snapshot.watches[0].rows[0].bridges.length);
  });

  it("links the verify:android failure to the job, not to the pipeline", () => {
    // 🔑 The claim the whole program exists to make: the website deployed AND
    // the android bridge failed, with the failing job one click away. The
    // pipeline has settled, and a failed bridge is open anyway.
    const link = [...host.querySelectorAll("a")].find(
      (a) => a.textContent?.trim() === "verify:android",
    );
    expect(link, "no link for verify:android").toBeTruthy();
    const href = link!.getAttribute("href") ?? "";
    expect(href).toMatch(/\/-\/jobs\/\d+$/);

    // The bridge that holds it is the failed one, not one of the three others.
    const row = link!.closest("[data-bridge]");
    expect(row?.getAttribute("data-bridge")).toBe("trigger:android");
    expect(row?.textContent).toContain("failed");
  });

  it("keeps a settled pipeline's passing bridges closed", () => {
    const main = host.querySelector('[data-watch="main-push"]')!;
    const expanded = (name: string) =>
      main
        .querySelector(`[data-bridge="${name}"] [data-slot="collapsible-trigger"]`)
        ?.getAttribute("aria-expanded");
    expect(expanded("trigger:website")).toBe("false");
    expect(expanded("trigger:ios")).toBe("false");
    // ...which is what makes the android assertion above a real distinction.
    expect(expanded("trigger:android")).toBe("true");
  });

  it("lists the parent pipeline's own jobs", () => {
    const main = host.querySelector('[data-watch="main-push"]')!;
    const parent = snapshot.watches[0].rows[0].parent_jobs[0];
    expect(main.querySelector(`[data-job-id="${parent.id}"]`)).toBeTruthy();
  });

  it("says when it last polled", () => {
    // "deployed with failure · 4m ago": the header is one line, so no "updated".
    expect(host.querySelector('[data-slot="updated-ago"]')?.textContent?.trim()).toMatch(/^\d+[smhd] ago$/);
  });

  it("keeps the secondary watches collapsed and unlabelled by state", () => {
    const hourly = host.querySelector('[data-watch="hourly"]');
    expect(hourly?.getAttribute("data-role")).toBe("secondary");
    // ⛔ A secondary watch has `icon_state: null` from the core and must never
    // be rendered with a state headline: that is the whole point of the role,
    // and an hourly schedule that is red by design would otherwise shout.
    expect(hourly?.querySelector("[data-headline]")).toBe(null);
    expect(hourly?.querySelector(".disclosure")?.getAttribute("aria-expanded")).toBe("false");
  });

  it("shows no error strip when the snapshot has no errors", () => {
    expect(snapshot.errors).toEqual([]);
    expect(host.querySelector('[role="alert"]')).toBe(null);
  });

  it("shows the request log in the debug pane", () => {
    expect(snapshot.request_log.length).toBeGreaterThan(0);
    const rows = host.querySelectorAll(".requests tbody tr");
    expect(rows.length).toBe(snapshot.request_log.length);
    expect(host.querySelector(".requests")?.textContent).toContain("GET");
  });
});

describe("Popover, the header and the footer", () => {
  let host: HTMLDivElement;
  let component: Record<string, unknown>;
  const handlers = {
    onrefresh: vi.fn(),
    onpipelines: vi.fn(),
    onsettings: vi.fn(),
    onquit: vi.fn(),
  };

  beforeEach(() => {
    for (const fn of Object.values(handlers)) fn.mockClear();
    host = document.createElement("div");
    document.body.append(host);
    component = mount(Popover, {
      target: host,
      props: { snapshot, now: Date.parse("2026-09-17T15:00:00Z"), ...handlers },
    });
    flushSync();
  });

  afterEach(() => {
    void unmount(component);
    host.remove();
  });

  const header = () => host.querySelector("header")!;
  const footer = () => host.querySelector<HTMLElement>('[data-slot="popover-footer"]')!;
  const named = (root: Element, name: string) =>
    [...root.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => (b.getAttribute("aria-label") ?? b.textContent?.trim()) === name,
    );

  it("has two icon buttons in the header, each named for a screen reader", () => {
    const buttons = [...header().querySelectorAll("button")];
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual(["Refresh", "Pipelines"]);
    for (const b of buttons) {
      // No visible words: the aria-label is the whole name, the icon is hidden.
      expect(b.textContent?.trim()).toBe("");
      expect(b.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
      expect(b.getAttribute("title")).toBeTruthy();
      expect(b.tabIndex).not.toBe(-1);
    }
    named(header(), "Refresh")!.click();
    named(header(), "Pipelines")!.click();
    expect(handlers.onrefresh).toHaveBeenCalledOnce();
    expect(handlers.onpipelines).toHaveBeenCalledOnce();
  });

  it("puts the state and the age on one line, with nothing else in words", () => {
    // "deployed with failure · 4m ago": the state, a dot and the age.
    const text = header().textContent?.replace(/\s+/g, " ").trim();
    expect(text).toMatch(/^deployed with failure · \d+[smhd] ago$/);
    expect(named(header(), "Settings")).toBeUndefined();
    expect(named(header(), "Quit")).toBeUndefined();
  });

  it("moves Settings and Quit to the footer", () => {
    expect([...footer().querySelectorAll("button")].map((b) => b.textContent?.trim())).toEqual([
      "Debug",
      "Settings…",
      "Quit",
    ]);
    named(footer(), "Settings…")!.click();
    named(footer(), "Quit")!.click();
    expect(handlers.onsettings).toHaveBeenCalledOnce();
    expect(handlers.onquit).toHaveBeenCalledOnce();
  });

  it("opens the debug pane from the footer, and says so to a screen reader", () => {
    const toggle = named(footer(), "Debug")!;
    const pane = host.querySelector<HTMLElement>(".debug")!;
    expect(toggle.getAttribute("aria-controls")).toBe(pane.id);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(pane.hidden).toBe(true);
    toggle.click();
    flushSync();
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(pane.hidden).toBe(false);
    toggle.click();
    flushSync();
    expect(pane.hidden).toBe(true);
  });
});

describe("Popover, degraded cases", () => {
  /** Mount, read, unmount. Every case below is one render with no interaction. */
  function render(s: Snapshot, read: (host: HTMLElement) => void) {
    const host = document.createElement("div");
    document.body.append(host);
    const component = mount(Popover, { target: host, props: { snapshot: s, now: 0 } });
    try {
      read(host);
    } finally {
      void unmount(component);
      host.remove();
    }
  }

  it("shows an error strip when the snapshot carries errors", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const broken: Snapshot = {
      ...snapshot,
      errors: ["main-push: rate limited; retry after 60s"],
    };
    const component = mount(Popover, { target: host, props: { snapshot: broken, now: 0 } });
    const alert = host.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("rate limited");
    void unmount(component);
    host.remove();
  });

  it("does not call a primary watch with no rows secondary", () => {
    // 🔑 A fresh install whose ref filter matches nothing. The header label is
    // the watch's ROLE, and the core sends `icon_state: null` here because
    // there is no pipeline to have a state.
    const fresh: Snapshot = {
      ...snapshot,
      icon_state: "unknown",
      watches: [{ id: "main-push", role: "primary", icon_state: null, rows: [], error: null }],
    };
    render(fresh, (host) => {
      const section = host.querySelector('[data-watch="main-push"]');
      expect(section?.getAttribute("data-role")).toBe("primary");
      expect(section?.querySelector(".role")).toBe(null);
      expect(section?.textContent).not.toContain("secondary");
      // ...and it says why it is empty rather than looking broken.
      expect(section?.textContent).toContain("no matching pipelines");
      expect(section?.querySelectorAll(".row").length).toBe(0);
      // A secondary watch with no state still carries the label, which is what
      // makes the assertions above a real distinction rather than a deletion.
      expect(host.querySelector('[data-watch="hourly"]')).toBe(null);
    });
  });

  it("shows a watch's own error instead of its rows", () => {
    const failing: Snapshot = {
      ...snapshot,
      watches: [
        {
          id: "main-push",
          role: "primary",
          icon_state: null,
          rows: [],
          error: "404 Project Not Found (project 12345678)",
        },
      ],
    };
    render(failing, (host) => {
      const section = host.querySelector('[data-watch="main-push"]');
      expect(section?.querySelector(".error")?.textContent).toContain("404 Project Not Found");
      // ⛔ "no matching pipelines" would be a lie here: nobody looked.
      expect(section?.textContent).not.toContain("no matching pipelines");
    });
  });

  it("shows a refused account's reason once, under its watches, not again in the strip", () => {
    // ⛔ The core lists every watch's error in `errors` too (for `check`), so
    // a host move refused for one account was said in the strip AND under each
    // of its watches.
    const reason = "account moved: the stored token of account \"moved\" was saved for https://gitlab.com";
    const refused: Snapshot = {
      ...snapshot,
      watches: [
        { id: "main-push", role: "primary", icon_state: null, rows: [], error: reason },
        { id: "hourly", role: "secondary", icon_state: null, rows: [], error: reason },
      ],
      errors: [`main-push: ${reason}`, `hourly: ${reason}`, "the poll loop crashed and was restarted: boom"],
    };
    render(refused, (host) => {
      const sections = [...host.querySelectorAll("[data-watch] .error")];
      expect(sections.map((e) => e.textContent)).toEqual([reason, reason]);
      const strip = [...host.querySelectorAll(".strip [data-line]")].map((l) => l.textContent);
      // An error no watch shows stays in the strip.
      expect(strip).toEqual(["the poll loop crashed and was restarted: boom"]);
      expect(host.textContent!.split(reason).length - 1).toBe(2);
    });
  });

  it("names the accounts whose prompt was cancelled, and Retry asks again", () => {
    const status: Status = {
      configPath: "/x/bridgewatch.toml",
      seeded: false,
      configOk: true,
      diagnostics: [],
      sinceLastPollSecs: 1,
      nextPollSecs: 4,
      launchAtLogin: false,
      platform: "macos",
      vibrancy: false,
      needsApproval: ["gl"],
    };
    const host = document.createElement("div");
    document.body.append(host);
    let retried = 0;
    const component = mount(Popover, {
      target: host,
      props: { snapshot, status, now: 0, onrefresh: () => retried++ },
    });
    try {
      const strip = host.querySelector('[data-slot="needs-approval"]');
      expect(strip?.textContent).toContain(
        "Account gl: the credential store's prompt was cancelled, so bridgewatch does not ask again on its own.",
      );
      strip?.querySelector<HTMLButtonElement>('[data-action="approval-retry"]')?.click();
      expect(retried).toBe(1);
    } finally {
      void unmount(component);
      host.remove();
    }
  });

  it("shows no approval strip when nothing waits for approval", () => {
    render(snapshot, (host) => {
      expect(host.querySelector('[data-slot="needs-approval"]')).toBe(null);
    });
  });

  it("keeps the secondary label on a secondary watch", () => {
    const quiet: Snapshot = {
      ...snapshot,
      watches: [{ id: "hourly", role: "secondary", icon_state: null, rows: [], error: null }],
    };
    render(quiet, (host) => {
      expect(host.querySelector('[data-watch="hourly"] .role')?.textContent).toBe("secondary");
    });
  });

  it("says so rather than rendering nothing when there are no watches", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const empty: Snapshot = {
      icon_state: "unknown",
      watches: [],
      errors: [],
      last_poll: "2026-09-17T15:00:00Z",
      request_log: [],
    };
    const component = mount(Popover, { target: host, props: { snapshot: empty, now: 0 } });
    expect(host.textContent).toContain("No watches configured");
    void unmount(component);
    host.remove();
  });

  it("does not blame an empty configuration when the file failed to load", () => {
    // ⛔ A config that does not parse yields no watches AND an error. Telling
    // the user to add a watch points them at the wrong file: the watches are
    // almost certainly already in the one the error names.
    const unparseable: Snapshot = {
      icon_state: "unknown",
      watches: [],
      errors: ["config: expected `=` after a key (line 12, column 3)"],
      last_poll: "2026-09-17T15:00:00Z",
      request_log: [],
    };
    render(unparseable, (host) => {
      expect(host.querySelector('[role="alert"]')?.textContent).toContain("expected `=`");
      expect(host.textContent).not.toContain("No watches configured");
    });
  });

  it("keeps the strip up from Status while the file does not load (H9)", () => {
    // ⛔ The review's case: a tick on the last good configuration publishes a
    // snapshot with NO errors while the file on disk still does not load. The
    // strip must not vanish; `Status` says the file is broken every second.
    const broken: Status = {
      configPath: "/tmp/bridgewatch.toml",
      seeded: false,
      configOk: false,
      diagnostics: [
        { severity: "error", path: "ui.jobs", message: "unknown variant `some`", line: 4, col: 8 },
      ],
      sinceLastPollSecs: 1,
      nextPollSecs: 4,
      launchAtLogin: false,
      platform: "macos",
      vibrancy: false,
    };
    const host = document.createElement("div");
    document.body.append(host);
    let fixed = 0;
    const component = mount(Popover, {
      target: host,
      props: {
        snapshot: { ...snapshot, errors: [] },
        status: broken,
        now: 0,
        onfix: () => fixed++,
      },
    });
    try {
      const alert = host.querySelector('[role="alert"]');
      expect(alert?.textContent).toContain("ui.jobs (line 4): unknown variant `some`");
      alert?.querySelector<HTMLButtonElement>("button")?.click();
      expect(fixed).toBe(1);
    } finally {
      void unmount(component);
      host.remove();
    }
  });

  it("shows the file's complaint once when the snapshot already carries it", () => {
    const status: Status = {
      configPath: "",
      seeded: false,
      configOk: false,
      diagnostics: [{ severity: "error", path: "ui.jobs", message: "nope", line: null, col: null }],
      sinceLastPollSecs: null,
      nextPollSecs: null,
      launchAtLogin: false,
      platform: "linux",
      vibrancy: false,
    };
    const host = document.createElement("div");
    document.body.append(host);
    const component = mount(Popover, {
      target: host,
      props: {
        snapshot: { ...snapshot, watches: [], errors: ["ui.jobs: nope"] },
        status,
        now: 0,
      },
    });
    try {
      const lines = [...host.querySelectorAll('[role="alert"] [data-line]')].map((l) => l.textContent);
      expect(lines).toEqual(["ui.jobs: nope"]);
      expect(host.textContent).not.toContain("No watches configured");
    } finally {
      void unmount(component);
      host.remove();
    }
  });

  it("keeps a bridge the user opened open across the next snapshot", async () => {
    const { flushSync } = await import("svelte");
    const { reactive } = await import("../lib/__tests__/props.svelte");
    const host = document.createElement("div");
    document.body.append(host);
    const props = reactive({ snapshot: structuredClone(snapshot), now: 0 });
    const component = mount(Popover, { target: host, props });
    flushSync();
    const trigger = () =>
      host.querySelector<HTMLButtonElement>(
        '[data-watch="main-push"] [data-bridge="trigger:website"] [data-slot="collapsible-trigger"]',
      )!;
    try {
      expect(trigger().getAttribute("aria-expanded")).toBe("false");
      trigger().click();
      flushSync();
      expect(trigger().getAttribute("aria-expanded")).toBe("true");
      // A poll replaces every object in the snapshot.
      props.snapshot = structuredClone(snapshot);
      flushSync();
      expect(trigger().getAttribute("aria-expanded")).toBe("true");
    } finally {
      void unmount(component);
      host.remove();
    }
  });

  it("points a first launch at the wizard rather than at a fix (no file yet)", () => {
    const first: Status = {
      configPath: "/home/u/.config/bridgewatch/config.toml",
      seeded: false,
      configOk: false,
      diagnostics: [
        {
          severity: "error",
          path: "",
          message:
            "No configuration yet. Finish the setup wizard, or choose \"I'll edit config.toml\" to write one yourself in Settings.",
          line: null,
          col: null,
        },
      ],
      sinceLastPollSecs: null,
      nextPollSecs: null,
      launchAtLogin: false,
      platform: "macos",
      vibrancy: false,
    };
    const host = document.createElement("div");
    document.body.append(host);
    let setup = 0;
    let fixed = 0;
    const component = mount(Popover, {
      target: host,
      props: {
        snapshot: { ...snapshot, watches: [], errors: [] },
        status: first,
        now: 0,
        onsetup: () => setup++,
        onfix: () => fixed++,
      },
    });
    try {
      const strip = host.querySelector('[role="alert"]')!;
      expect(strip.getAttribute("data-first-run")).toBe("true");
      expect(strip.textContent).toContain("No configuration yet.");
      expect(strip.querySelector("button.fix")).toBe(null);
      strip.querySelector<HTMLButtonElement>("button.setup")!.click();
      expect([setup, fixed]).toEqual([1, 0]);
      expect(host.textContent).toContain("setup wizard");
      expect(host.textContent).not.toContain("until the problem above is fixed");
    } finally {
      void unmount(component);
      host.remove();
    }
  });
});
