import { describe, expect, it } from "vitest";

import {
  PANE_GROUPS,
  PANE_KEY,
  PANES,
  WATCH_GROUPS,
  accountGroup,
  paneFor,
  rememberPane,
  restorePane,
  watchGroup,
} from "./panes";
import { REGISTRY, entriesFor, type Tab } from "./registry";

describe("panes", () => {
  it("are the five the sidebar shows, in order", () => {
    expect(PANES.map((p) => p.id)).toEqual(["accounts", "watches", "general", "verdict", "advanced"]);
  });

  it("map every old tab name the shell may send", () => {
    expect(paneFor("accounts")).toEqual({ pane: "accounts", focus: null });
    expect(paneFor("icon")?.pane).toBe("general");
    expect(paneFor("ui")?.pane).toBe("general");
    expect(paneFor("log")).toEqual({ pane: "advanced", focus: null });
    expect(paneFor("text")).toEqual({ pane: "advanced", focus: "text" });
    expect(paneFor("advanced")).toEqual({ pane: "advanced", focus: null });
    expect(paneFor("nonsense")).toBe(null);
    expect(paneFor(null)).toBe(null);
    expect(paneFor(undefined)).toBe(null);
  });

  it("show every registry tab somewhere, so no key loses its place in the window", () => {
    // Accounts and Watches have panes of their own; Advanced renders `log`.
    const shown = new Set<Tab>(["accounts", "watches", "log"]);
    for (const groups of Object.values(PANE_GROUPS)) for (const g of groups) shown.add(g.tab);
    for (const entry of REGISTRY) expect(shown.has(entry.tab), entry.path).toBe(true);
  });

  it("restore the last pane, and Accounts when there is none or storage refuses", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    };
    expect(restorePane(storage)).toBe("accounts");
    rememberPane(storage, "verdict");
    expect(store.get(PANE_KEY)).toBe("verdict");
    expect(restorePane(storage)).toBe("verdict");
    // Something stored by an older or newer build that this one does not know.
    store.set(PANE_KEY, "sidebar-v9");
    expect(restorePane(storage)).toBe("accounts");
    const refusing = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(restorePane(refusing)).toBe("accounts");
    expect(() => rememberPane(refusing, "watches")).not.toThrow();
    expect(restorePane(null)).toBe("accounts");
    expect(() => rememberPane(undefined, "watches")).not.toThrow();
  });
});

describe("groups", () => {
  it("place every account key, sign-in first", () => {
    // ⛔ A new account key must be given a group here, not silently land in Advanced.
    for (const e of entriesFor("accounts")) expect(accountGroup(e.path), e.path).not.toBe(null);
    expect(accountGroup("accounts.*.token")).toBe("Sign-in");
    expect(accountGroup("accounts.*.base_url")).toBe("Server");
    expect(accountGroup("accounts.*.rate_limit_backoff.max_secs")).toBe("Advanced");
    expect(accountGroup("watches.*.id")).toBe(null);
    expect(accountGroup("accounts.*.nope")).toBe(null);
  });

  it("place every watch key by its config path", () => {
    // ⛔ A new watch key fails here with its path named until it is given a group.
    for (const e of entriesFor("watches")) expect(watchGroup(e.path), e.path).not.toBe(null);
    expect(watchGroup("watches.*.show.max_rows")).toBe("Rows");
    expect(watchGroup("watches.*.poll.live_secs")).toBe("Polling");
    expect(watchGroup("watches.*.dive.bridges")).toBe("Child pipelines");
    expect(watchGroup("watches.*.jobs.*")).toBe("Jobs");
    expect(watchGroup("watches.*.notify.recovered")).toBe("Notifications");
    expect(watchGroup("watches.*.notify.click")).toBe("Notifications");
    expect(watchGroup("watches.*.deploy_markers")).toBe("Deploy");
    expect(watchGroup("watches.*.ref")).toBe("Source");
    expect(watchGroup("watches.*.nope")).toBe(null);
    expect(watchGroup("watches.*.nope.deeper")).toBe(null);
    expect(watchGroup("accounts.*.token")).toBe(null);
  });

  it("are seven, in the order the pane shows them, and each one has keys", () => {
    expect(WATCH_GROUPS).toEqual(["Source", "Rows", "Polling", "Child pipelines", "Deploy", "Jobs", "Notifications"]);
    for (const g of WATCH_GROUPS) {
      expect(entriesFor("watches").some((e) => watchGroup(e.path) === g), g).toBe(true);
    }
  });
});
