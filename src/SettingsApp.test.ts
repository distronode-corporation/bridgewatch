import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { Edit, Status, Validation } from "./lib/types";

/**
 * The settings window's own behaviour: what it asks, and what it shows without
 * being asked.
 *
 * Both defects here are about a window that says nothing. One asked its
 * questions through `window.confirm`/`window.prompt`, which never appear in
 * this webview, so two buttons did nothing; the other opened on a file that
 * does not load and showed no reason until the user found "Validate".
 */

const OK: Validation = { ok: true, diagnostics: [] };

const BROKEN: Status = {
  configPath: "/tmp/bridgewatch.toml",
  seeded: false,
  configOk: false,
  diagnostics: [
    {
      severity: "error",
      path: "watches[0].project",
      message: "expected `=` after a key",
      line: 12,
      col: 3,
    },
  ],
  sinceLastPollSecs: null,
  nextPollSecs: null,
  launchAtLogin: false,
  platform: "macos",
  vibrancy: false,
};

const HEALTHY: Status = { ...BROKEN, configOk: true, diagnostics: [] };

const CONFIG = {
  accounts: { "gitlab.com": { base_url: "https://gitlab.com", token: { own: true } } },
  watches: [
    { id: "main-push", role: "primary", account: "gitlab.com", project: 12345678, ref: "main" },
    { id: "hourly", role: "secondary", account: "gitlab.com", project: 12345678, ref: "main" },
  ],
};

const shell = {
  status: HEALTHY,
  tab: null as string | null,
  edits: [] as Edit[][],
  confirms: [] as (string | null)[],
  /** Answers handed out before falling back to `result`, in order. */
  queue: [] as Validation[],
  result: OK,
  /** What `get_config_json` answers: the OS decides `ui.launch_at_login`. */
  config: CONFIG as Record<string, unknown>,
  text: '[accounts."gitlab.com"]\n',
  moves: [] as [string, number][],
  moveResult: OK,
  saves: [] as { text: string; base: string; confirm: string | null }[],
  saveQueue: [] as Validation[],
  /** The `config-changed` listener SettingsApp registered, if any. */
  changed: null as null | (() => void),
  wizardOpened: 0,
  /** What `open_config_file` and `open_wizard` reject with, if anything. */
  openError: null as string | null,
};

vi.mock("./lib/ipc", () => ({
  inTauri: () => true,
  // Sign-in is not what these tests are about: this build offers it nowhere
  // (no client id is built in), so the token field shows the four sources.
  oauthApi: () => ({
    availability: () =>
      Promise.resolve({ available: false, builtin: false, needs_client_id: false, host: "gitlab.com", install_url: null }),
    status: () => Promise.resolve(null),
  }),
  ownTokenStatus: () => Promise.resolve({ kind: "none" }),
  getConfigJson: () => Promise.resolve({ config: shell.config, job_order: [[], []] }),
  readConfigText: () => Promise.resolve(shell.text),
  getStatus: () => Promise.resolve(shell.status),
  takeSettingsTab: () => Promise.resolve(shell.tab),
  onOpenSettings: () => Promise.resolve(() => {}),
  applyConfigEdits: (edits: Edit[], confirm?: string) => {
    shell.edits.push(edits);
    shell.confirms.push(confirm ?? null);
    const next = shell.queue.shift();
    return Promise.resolve(next ?? shell.result);
  },
  onConfigChanged: (handler: () => void) => {
    shell.changed = handler;
    return Promise.resolve(() => {});
  },
  moveWatch: (id: string, delta: number) => {
    shell.moves.push([id, delta]);
    return Promise.resolve(shell.moveResult);
  },
  openConfigFile: () => (shell.openError ? Promise.reject(shell.openError) : Promise.resolve()),
  openWizard: () => {
    shell.wizardOpened++;
    return shell.openError ? Promise.reject(shell.openError) : Promise.resolve();
  },
  reloadConfig: () => Promise.resolve(),
  saveConfigText: (text: string, base: string, confirm?: string) => {
    shell.saves.push({ text, base, confirm: confirm ?? null });
    return Promise.resolve(shell.saveQueue.shift() ?? OK);
  },
  validateConfigText: () => Promise.resolve(OK),
}));

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;
let SettingsApp: typeof import("./SettingsApp.svelte").default;

// ⚠ Imported once, here, as PopoverApp.test.ts does: a cold import compiles the
// whole settings tree, and inside the first test that ran into the 5 s test
// timeout on a loaded machine (seen 2026-10-02 in a full parallel run).
beforeAll(async () => {
  ({ default: SettingsApp } = await import("./SettingsApp.svelte"));
}, 120_000);

beforeEach(() => {
  // The window restores the last pane used: every test starts from none.
  localStorage.clear();
  shell.status = HEALTHY;
  shell.tab = null;
  shell.edits = [];
  shell.confirms = [];
  shell.queue = [];
  shell.result = OK;
  shell.config = CONFIG;
  shell.text = '[accounts."gitlab.com"]\n';
  shell.moves = [];
  shell.moveResult = OK;
  shell.saves = [];
  shell.saveQueue = [];
  shell.changed = null;
  shell.wizardOpened = 0;
  shell.openError = null;
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

/** Mount, and let the mount-time IPC round trips resolve. */
async function render() {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(SettingsApp, { target: host, props: {} });
  await settle();
  return {
    tab: async (label: string) => {
      [...host.querySelectorAll<HTMLButtonElement>("button.tab")]
        .find((b) => b.textContent?.trim() === label)!
        .click();
      await settle();
    },
    click: async (selector: string, index = 0) => {
      host.querySelectorAll<HTMLButtonElement>(selector)[index].click();
      await settle();
    },
    // ⚠ By text: the watch list's "Add watch" and the job-override editor's
    // "Add row" are both `button.add`, and the row editor is rendered first
    // because the open watch's body sits above the list's footer.
    clickText: async (label: string) => {
      [...host.querySelectorAll<HTMLButtonElement>("button")]
        .find((b) => b.textContent?.trim() === label)!
        .click();
      await settle();
    },
    ask: () => host.querySelector('[role="dialog"]'),
    fill: (index: number, value: string) => {
      const input = host.querySelectorAll<HTMLInputElement>('[role="dialog"] input')[index];
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      flushSync();
    },
    diagnostics: () => host.querySelector(".diagnostics")?.textContent ?? "",
  };
}

async function settle() {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  flushSync();
}

describe("SettingsApp, a configuration that does not load", () => {
  it("shows the diagnostics without being asked to validate", async () => {
    // ⛔ The window opens itself on the text tab for a broken file, with the
    // TOML in the box and nothing saying what is wrong with it. `Status` has
    // carried `configOk` and the diagnostics since the first version; nothing
    // rendered them, so the user had to guess that a button labelled Validate
    // was how you find out.
    shell.status = BROKEN;
    await render();
    expect(host.textContent).toContain("expected `=` after a key");
    expect(host.textContent).toContain("12:3");
  });

  it("shows them on the text tab too, which is the one it opens on", async () => {
    shell.status = BROKEN;
    shell.tab = "text";
    await render();
    expect(host.querySelector("textarea.editor")).toBeTruthy();
    expect(host.textContent).toContain("expected `=` after a key");
  });

  it("says nothing when the file is fine", async () => {
    const h = await render();
    expect(h.diagnostics()).toBe("");
  });
});

describe("SettingsApp, the questions it asks", () => {
  it("confirms a removal in the document, and writes nothing until it is answered", async () => {
    // ⛔ `window.confirm` returns `false` immediately in this webview — wry
    // implements no `WKUIDelegate` panel methods — so "Remove watch" was a
    // button that did nothing at all.
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="remove this watch"]', 1);
    expect(shell.edits).toEqual([]);
    expect(h.ask()?.textContent).toContain("hourly");
    await h.click('[role="dialog"] .confirm');
    expect(shell.edits).toEqual([[{ op: "remove_watch", id: "hourly" }]]);
    expect(h.ask()).toBe(null);
  });

  it("removes nothing when the question is declined", async () => {
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="remove this watch"]', 0);
    await h.click('[role="dialog"] .cancel');
    expect(shell.edits).toEqual([]);
    expect(h.ask()).toBe(null);
  });

  it("collects the new watch's id and project in the document", async () => {
    // Two `window.prompt` calls, both of which returned `null` here, so "Add
    // watch" was dead in exactly the same way.
    const h = await render();
    await h.tab("Watches");
    await h.clickText("Add watch");
    expect(shell.edits).toEqual([]);
    h.fill(0, "preflights");
    h.fill(1, "12345678");
    await h.click('[role="dialog"] .confirm');
    expect(shell.edits).toEqual([
      [
        {
          op: "add_watch",
          watch: {
            id: "preflights",
            account: "gitlab.com",
            // A numeric id is written as a number, which is what the core's
            // `project` union prefers.
            project: 12345678,
            ref: "main",
            role: "secondary",
          },
        },
      ],
    ]);
  });

  it("will not add a watch with half an answer", async () => {
    const h = await render();
    await h.tab("Watches");
    await h.clickText("Add watch");
    h.fill(0, "preflights");
    const confirm = host.querySelector<HTMLButtonElement>('[role="dialog"] .confirm')!;
    expect(confirm.disabled).toBe(true);
    confirm.click();
    await settle();
    expect(shell.edits).toEqual([]);
  });

  it("takes a group/path project as a string", async () => {
    const h = await render();
    await h.tab("Watches");
    await h.clickText("Add watch");
    h.fill(0, "other");
    h.fill(1, " distronode-corporation/bridgewatch ");
    await h.click('[role="dialog"] .confirm');
    const watch = (shell.edits[0][0] as { watch: Record<string, unknown> }).watch;
    expect(watch.project).toBe("distronode-corporation/bridgewatch");
  });

  it("keeps the reason a refused save was refused", async () => {
    // ⚠ The reload that follows every save re-seeds the diagnostics from the
    // file on disk — which is unchanged, and therefore fine — so re-seeding
    // unconditionally would replace "here is why your edit was rejected" with
    // silence.
    shell.result = {
      ok: false,
      diagnostics: [
        {
          severity: "error",
          path: "watches[1]",
          message: "unknown field `rol`",
          line: null,
          col: null,
        },
      ],
    };
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="remove this watch"]', 1);
    await h.click('[role="dialog"] .confirm');
    expect(h.diagnostics()).toContain("unknown field `rol`");
  });
});

describe("SettingsApp, a change the shell wants confirmed", () => {
  const ASKED: Validation = {
    ok: false,
    diagnostics: [],
    confirm: {
      id: "c-1",
      changes: ['Account "gitlab.com" will get its token by running `pass gl`.'],
    },
  };

  it("shows the shell's own words and resends with its id on yes", async () => {
    // Lo13: the first answer wrote nothing. Only the same edits sent back with
    // the id the shell issued are written.
    shell.queue = [ASKED];
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="remove this watch"]', 1);
    await h.click('[role="dialog"] .confirm');
    expect(h.ask()?.textContent).toContain("`pass gl`");
    expect(shell.confirms).toEqual([null]);
    await h.click('[role="dialog"] .confirm');
    expect(shell.confirms).toEqual([null, "c-1"]);
    expect(shell.edits[1]).toEqual(shell.edits[0]);
    expect(h.ask()).toBe(null);
  });

  it("sends nothing more on no", async () => {
    shell.queue = [ASKED];
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="remove this watch"]', 1);
    await h.click('[role="dialog"] .confirm');
    await h.click('[role="dialog"] .cancel');
    expect(shell.confirms).toEqual([null]);
    expect(host.textContent).toContain("not confirmed");
  });
});

describe("SettingsApp, moving a watch (H11)", () => {
  it("asks the shell to move THAT watch, and shows the file's new order", async () => {
    const h = await render();
    await h.tab("Watches");
    // The shell renumbers the watches and the window re-reads the file.
    shell.config = { ...CONFIG, watches: [CONFIG.watches[1], CONFIG.watches[0]] };
    await h.click('button[title="move down"]', 0);
    expect(shell.moves).toEqual([["main-push", 1]]);
    const order = [...host.querySelectorAll("section.watch header strong")].map((s) => s.textContent);
    expect(order).toEqual(["hourly", "main-push"]);
  });

  it("moves up with a negative delta", async () => {
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="move up"]', 1);
    expect(shell.moves).toEqual([["hourly", -1]]);
  });

  it("says why a move was refused instead of doing nothing", async () => {
    shell.moveResult = {
      ok: false,
      diagnostics: [
        { severity: "error", path: "watches", message: "the file changed on disk", line: null, col: null },
      ],
    };
    const h = await render();
    await h.tab("Watches");
    await h.click('button[title="move down"]', 0);
    expect(h.diagnostics()).toContain("the file changed on disk");
    expect(host.querySelector(".notice")?.textContent).toContain("Not moved");
  });
});

describe("SettingsApp, launch at login (H13)", () => {
  function checkbox(): HTMLInputElement {
    const label = [...host.querySelectorAll("label.field")].find((l) =>
      l.textContent?.includes("Launch at login"),
    )!;
    return label.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
  }

  it("writes the key the shell routes to the OS login item", async () => {
    const h = await render();
    await h.tab("General");
    const box = checkbox();
    expect(box.checked).toBe(false);
    box.click();
    await settle();
    expect(shell.edits).toEqual([
      [{ op: "set", path: "ui.launch_at_login", value: { boolean: true } }],
    ]);
  });

  it("shows what the OS did, not what was clicked, when the OS refused", async () => {
    // The shell saves the file, fails to register the login item, answers
    // `ok: false` with the reason, and `get_config_json` reports the OS (off).
    shell.result = {
      ok: false,
      diagnostics: [
        {
          severity: "error",
          path: "ui.launch_at_login",
          message: "the file was saved, but the login item could not be changed: denied",
          line: null,
          col: null,
        },
      ],
    };
    const h = await render();
    await h.tab("General");
    checkbox().click();
    await settle();
    await settle();
    expect(checkbox().checked).toBe(false);
    expect(h.diagnostics()).toContain("login item could not be changed");
  });

  it("follows the OS state the shell reports", async () => {
    shell.config = { ...CONFIG, ui: { launch_at_login: true } };
    const h = await render();
    await h.tab("General");
    expect(checkbox().checked).toBe(true);
  });
});

describe("SettingsApp, the file changing underneath it (H15)", () => {
  it("re-reads the file on the shell's config-changed event", async () => {
    shell.tab = "text";
    await render();
    const editor = () => host.querySelector<HTMLTextAreaElement>("textarea.editor")!;
    expect(editor().value).toBe('[accounts."gitlab.com"]\n');
    expect(shell.changed).toBeTypeOf("function");
    shell.text = "# edited in $EDITOR\n";
    shell.changed!();
    await settle();
    expect(editor().value).toBe("# edited in $EDITOR\n");
  });

  it("saves against the text it was seeded from, and says so on a conflict", async () => {
    shell.tab = "text";
    shell.saveQueue = [{ ok: false, diagnostics: [], conflict: true }];
    await render();
    const editor = host.querySelector<HTMLTextAreaElement>("textarea.editor")!;
    editor.value = "# mine\n";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    host.querySelector<HTMLButtonElement>("button.save")!.click();
    await settle();
    expect(shell.saves).toEqual([
      { text: "# mine\n", base: '[accounts."gitlab.com"]\n', confirm: null },
    ]);
    expect(host.textContent).toContain("changed on disk");
    // The draft is kept: nothing was written.
    expect(host.querySelector<HTMLTextAreaElement>("textarea.editor")!.value).toBe("# mine\n");
  });

  it("drops a save's 'valid' when another writer then breaks the file", async () => {
    // ⛔ The verdict of this window's own save is kept across the
    // config-changed event that save causes. It used to be kept across EVERY
    // later one too, so a file broken in $EDITOR after a good save still read
    // "valid", with no diagnostics.
    shell.tab = "text";
    await render();
    const editor = host.querySelector<HTMLTextAreaElement>("textarea.editor")!;
    editor.value = "# mine\n";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    host.querySelector<HTMLButtonElement>("button.save")!.click();
    await settle();
    // The shell announces this window's own write: the verdict stays.
    shell.text = "# mine\n";
    shell.changed!();
    await settle();
    expect(host.querySelector(".verdict")?.textContent?.trim()).toBe("valid");
    // $EDITOR breaks the file; the shell reloads, fails, and says so.
    shell.text = "# mine\nbroken = \n";
    shell.status = BROKEN;
    shell.changed!();
    await settle();
    expect(host.querySelector(".verdict")?.textContent?.trim()).not.toBe("valid");
    expect(host.textContent).toContain("expected `=` after a key");
  });

  it("asks the shell's question for a text save too, and resends with the id (Lo13)", async () => {
    shell.tab = "text";
    shell.saveQueue = [
      { ok: false, diagnostics: [], confirm: { id: "c-9", changes: ["runs `pass gl`"] } },
    ];
    const h = await render();
    const editor = host.querySelector<HTMLTextAreaElement>("textarea.editor")!;
    editor.value = "# with a command\n";
    editor.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    host.querySelector<HTMLButtonElement>("button.save")!.click();
    await settle();
    expect(h.ask()?.textContent).toContain("`pass gl`");
    await h.click('[role="dialog"] .confirm');
    expect(shell.saves.map((s) => s.confirm)).toEqual([null, "c-9"]);
  });
});

describe("SettingsApp, the setup wizard", () => {
  it("has an entry that asks the shell to open the wizard window", async () => {
    const h = await render();
    await h.clickText("Setup wizard…");
    expect(shell.wizardOpened).toBe(1);
    // Opening the wizard writes nothing by itself.
    expect(shell.edits).toEqual([]);
  });
});

describe("SettingsApp, opening things the shell may fail to open", () => {
  it("says why 'Open file' did nothing", async () => {
    // A desktop with no `.toml` handler (common on Linux), or no file yet:
    // the rejection used to be dropped, so the click did nothing at all.
    shell.openError = "no application is registered for .toml files";
    const h = await render();
    await h.clickText("Open file");
    expect(host.querySelector(".notice")?.textContent).toContain(
      "Could not open the file: no application is registered for .toml files",
    );
  });

  it("says why the wizard did not open", async () => {
    shell.openError = "the wizard window could not be created";
    const h = await render();
    await h.clickText("Setup wizard…");
    expect(host.querySelector(".notice")?.textContent).toContain(
      "Could not open the setup wizard: the wizard window could not be created",
    );
  });
});

describe("SettingsApp, a first launch with no file yet", () => {
  it("offers the wizard instead of listing the missing file as an error", async () => {
    shell.status = {
      ...BROKEN,
      diagnostics: [
        {
          severity: "error",
          path: "",
          message: "No configuration yet. Finish the setup wizard, or choose \"I'll edit config.toml\".",
          line: null,
          col: null,
        },
      ],
    };
    shell.config = null as unknown as Record<string, unknown>;
    shell.text = "";
    const h = await render();
    expect(h.diagnostics()).toBe("");
    expect(host.querySelector(".first-run")?.textContent).toContain("No configuration yet");
    await h.clickText("Open the setup wizard");
    expect(shell.wizardOpened).toBe(1);
  });

  it("uses the shell's firstRun flag when it sends one", async () => {
    shell.status = { ...BROKEN, firstRun: true };
    await render();
    expect(host.querySelector(".first-run")).toBeTruthy();
  });
});

describe("SettingsApp, the sidebar (L2)", () => {
  const tabs = () => [...host.querySelectorAll<HTMLButtonElement>('[role="tablist"] [role="tab"]')];
  const selected = () => tabs().find((t) => t.getAttribute("aria-selected") === "true")?.textContent?.trim();
  const title = () => host.querySelector("header h2")?.textContent?.trim();

  it("lists five panes in a vertical tab list, Accounts first", async () => {
    await render();
    const list = host.querySelector('[role="tablist"]')!;
    expect(list.getAttribute("aria-orientation")).toBe("vertical");
    expect(tabs().map((t) => t.textContent?.trim())).toEqual(["Accounts", "Watches", "General", "Verdict", "Advanced"]);
    expect(selected()).toBe("Accounts");
    expect(title()).toBe("Accounts");
    // Only the selected tab is in the Tab order, and the pane names it.
    expect(tabs().map((t) => t.tabIndex)).toEqual([0, -1, -1, -1, -1]);
    const panel = host.querySelector('[role="tabpanel"]')!;
    expect(panel.getAttribute("aria-labelledby")).toBe(tabs()[0].id);
    expect(tabs()[0].getAttribute("aria-controls")).toBe(panel.id);
  });

  it("moves with the arrow keys, Home and End, wrapping, and takes the focus along", async () => {
    await render();
    const key = async (k: string) => {
      (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
      await settle();
    };
    tabs()[0].focus();
    await key("ArrowDown");
    expect(selected()).toBe("Watches");
    expect(document.activeElement).toBe(tabs()[1]);
    await key("End");
    expect(selected()).toBe("Advanced");
    await key("ArrowDown");
    expect(selected()).toBe("Accounts");
    await key("ArrowUp");
    expect(selected()).toBe("Advanced");
    await key("Home");
    expect(selected()).toBe("Accounts");
    expect(document.activeElement).toBe(tabs()[0]);
    expect(title()).toBe("Accounts");
  });

  it("opens on the last pane used", async () => {
    const h = await render();
    await h.tab("Verdict");
    void unmount(component!);
    component = null;
    host.remove();
    await render();
    expect(selected()).toBe("Verdict");
    expect(host.textContent).toContain("Inline script");
  });

  it("opens on Accounts when the window's storage refuses", async () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    try {
      const h = await render();
      expect(selected()).toBe("Accounts");
      // A refused write is not an error either: the pane still changes.
      await h.tab("Watches");
      expect(selected()).toBe("Watches");
    } finally {
      get.mockRestore();
      set.mockRestore();
    }
  });

  it("shows the shell's 'text' as Advanced with the editor, and does not remember it", async () => {
    // The shell still names the old tabs: a broken file and the wizard's
    // "I'll edit config.toml" both ask for "text".
    shell.tab = "text";
    await render();
    expect(selected()).toBe("Advanced");
    expect(host.querySelector("#edit-as-text textarea.editor")).toBeTruthy();
    expect(localStorage.getItem("bridgewatch.settings.pane")).toBe(null);
  });

  it.each([
    ["icon", "General"],
    ["ui", "General"],
    ["log", "Advanced"],
    ["watches", "Watches"],
  ])("maps the shell's old tab %s to %s", async (tab, pane) => {
    shell.tab = tab;
    await render();
    expect(selected()).toBe(pane);
  });

  it("keeps the pane it has for a name it does not know", async () => {
    shell.tab = "nonsense";
    await render();
    expect(selected()).toBe("Accounts");
  });

  it("puts General's two groups and Advanced's log and editor under their titles", async () => {
    const h = await render();
    await h.tab("General");
    const groups = () => [...host.querySelectorAll(".pane .settings-group h3")].map((e) => e.textContent?.trim());
    expect(groups()).toEqual(["Icon", "App"]);
    expect(host.textContent).toContain("Launch at login");
    expect(host.textContent).toContain("State glyphs");
    await h.tab("Advanced");
    expect(groups()).toEqual(["Log", "Edit as text"]);
    expect(host.querySelector("textarea.editor")).toBeTruthy();
  });

  it("keeps the file path, Open file and Reload in the footer bar", async () => {
    await render();
    const footer = host.querySelector("footer")!;
    expect(footer.querySelector(".path")?.textContent).toBe("/tmp/bridgewatch.toml");
    expect([...footer.querySelectorAll("button")].map((b) => b.textContent?.trim())).toEqual(["Open file", "Reload"]);
  });

  it("puts Add watch in the Watches pane's header, and only there", async () => {
    const h = await render();
    expect(host.querySelector("header")?.textContent).not.toContain("Add watch");
    await h.tab("Watches");
    expect(host.querySelector("header")?.textContent).toContain("Add watch");
  });
});

describe("SettingsApp, a refused form edit on the Advanced pane (Seer, #65)", () => {
  it("lists the reason under the pane, where it is seen, and not under the editor", async () => {
    // ⛔ The editor lists diagnostics under its 340px box. A refused Log edit
    // put its reason there, out of sight, while the notice said "see the
    // errors below".
    shell.result = {
      ok: false,
      diagnostics: [{ severity: "error", path: "log.level", message: "unknown variant `loud`", line: null, col: null }],
    };
    const h = await render();
    await h.tab("Advanced");
    const select = [...host.querySelectorAll<HTMLLabelElement>("label.field")]
      .find((l) => l.textContent?.includes("Log level"))!
      .querySelector("select")!;
    select.value = "trace";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    await settle();
    const outer = host.querySelector(".main > .diagnostics");
    expect(outer?.textContent).toContain("unknown variant `loud`");
    expect(host.querySelector("#edit-as-text")?.textContent).not.toContain("unknown variant `loud`");
  });

  it("still lists a broken file's diagnostics under the editor there, once", async () => {
    shell.status = BROKEN;
    shell.tab = "text";
    await render();
    expect(host.querySelector("#edit-as-text")?.textContent).toContain("expected `=` after a key");
    expect(host.querySelector(".main > .diagnostics")).toBe(null);
  });
});

describe("SettingsApp, keyboard and screen-reader wiring (cubic, #65)", () => {
  const tabs = () => [...host.querySelectorAll<HTMLButtonElement>('[role="tablist"] [role="tab"]')];

  /** Every element Tab stops on, in document order (jsdom has no layout, so this is the DOM's order). */
  const tabOrder = () =>
    [...host.querySelectorAll<HTMLElement>("button, input, select, textarea, a[href], [tabindex]")].filter(
      (el) => el.tabIndex >= 0 && !(el as HTMLButtonElement).disabled,
    );

  it("never names a panel that does not exist, on any pane", async () => {
    // ⛔ Only the shown pane is mounted. Every tab carried aria-controls, so
    // four of them pointed at nothing.
    const h = await render();
    for (const label of ["Accounts", "Watches", "General", "Verdict", "Advanced"]) {
      await h.tab(label);
      const controls = tabs().filter((t) => t.hasAttribute("aria-controls"));
      expect(controls.map((t) => t.textContent?.trim()), label).toEqual([label]);
      for (const t of controls) {
        const panel = document.getElementById(t.getAttribute("aria-controls")!);
        expect(panel, label).toBeTruthy();
        expect(panel!.getAttribute("role")).toBe("tabpanel");
        expect(panel!.getAttribute("aria-labelledby")).toBe(t.id);
      }
    }
  });

  it("gives an empty Watches pane something to Tab to, after the header and before the footer", async () => {
    // ⛔ With only text in the empty pane, Tab went from the header's Add watch
    // straight to the footer: the pane, and what it says, were never reached.
    shell.config = { ...CONFIG, watches: [] };
    const h = await render();
    await h.tab("Watches");
    const order = tabOrder();
    const panel = host.querySelector('[role="tabpanel"]')!;
    const inPanel = order.filter((el) => panel.contains(el));
    expect(inPanel.map((el) => el.textContent?.trim())).toEqual(["Add watch"]);
    const at = (el: Element) => order.indexOf(el as HTMLElement);
    const header = host.querySelector<HTMLElement>("header button.add")!;
    const footer = host.querySelector<HTMLElement>("footer button")!;
    expect(at(header)).toBeLessThan(at(inPanel[0]));
    expect(at(inPanel[0])).toBeLessThan(at(footer));
    expect(panel.textContent).toContain("No watches yet");
    inPanel[0].click();
    await settle();
    expect(h.ask()?.textContent).toContain("New watch");
  });
});
