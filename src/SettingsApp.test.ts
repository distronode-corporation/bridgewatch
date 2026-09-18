import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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
    { id: "main-push", role: "primary", account: "gitlab.com", project: 82468124, ref: "main" },
    { id: "hourly", role: "secondary", account: "gitlab.com", project: 82468124, ref: "main" },
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
};

vi.mock("./lib/ipc", () => ({
  inTauri: () => true,
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
  openConfigFile: () => Promise.resolve(),
  openWizard: () => {
    shell.wizardOpened++;
    return Promise.resolve();
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

beforeEach(() => {
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
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

/** Mount, and let the mount-time IPC round trips resolve. */
async function render() {
  const { default: SettingsApp } = await import("./SettingsApp.svelte");
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
    h.fill(1, "82468124");
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
            project: 82468124,
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
    await h.tab("UI");
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
    await h.tab("UI");
    checkbox().click();
    await settle();
    await settle();
    expect(checkbox().checked).toBe(false);
    expect(h.diagnostics()).toContain("login item could not be changed");
  });

  it("follows the OS state the shell reports", async () => {
    shell.config = { ...CONFIG, ui: { launch_at_login: true } };
    const h = await render();
    await h.tab("UI");
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
