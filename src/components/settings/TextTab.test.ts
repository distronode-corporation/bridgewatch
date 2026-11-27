// SPDX-FileCopyrightText: Distronode Corporation
//
// SPDX-License-Identifier: Apache-2.0

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { Validation } from "../../lib/types";
import TextTab from "./TextTab.svelte";

/**
 * "Edit as text": the tab that holds the user's own words.
 *
 * Both defects here destroy work rather than merely misreporting it, and
 * neither is visible on a first render — one needs a REFUSED save, the other
 * needs the file to have moved underneath the buffer.
 */

const OK: Validation = { ok: true, diagnostics: [] };
const BAD: Validation = {
  ok: false,
  diagnostics: [
    { severity: "error", path: "", message: "expected `=` after a key", line: 12, col: 3 },
  ],
};

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function render(text: string, verdict: Validation, onDisk = () => text) {
  host = document.createElement("div");
  document.body.append(host);
  const saved: string[] = [];
  const reverts: number[] = [];
  const props = reactive({
    text,
    validation: null,
    diskText: () => Promise.resolve(onDisk()),
    onvalidate: () => {},
    onsave: (t: string, base: string) => {
      // What the shell does (H15): a compare-and-swap against the text the
      // buffer was seeded from. A moved file writes nothing.
      if (base !== onDisk()) {
        return Promise.resolve({ ok: false, diagnostics: [], conflict: true } as Validation);
      }
      saved.push(t);
      // What the real handler does: the parent re-reads the file, so `text`
      // moves to whatever is on disk NOW — the new text on a save that landed,
      // the old text on one that did not.
      if (verdict.ok) props.text = t;
      props.validation = verdict;
      return Promise.resolve(verdict);
    },
    onrevert: () => reverts.push(1),
  }) as ComponentProps<typeof TextTab>;
  component = mount(TextTab, { target: host, props });
  flushSync();
  return {
    props,
    saved,
    reverts,
    box: () => host.querySelector("textarea")!,
    type: (value: string) => {
      const box = host.querySelector("textarea")!;
      box.value = value;
      box.dispatchEvent(new Event("input", { bubbles: true }));
      flushSync();
    },
    save: async () => {
      host.querySelector<HTMLButtonElement>("button.save")!.click();
      await settle();
    },
    overwrite: async () => {
      host.querySelector<HTMLButtonElement>("button.overwrite")!.click();
      await settle();
    },
    conflict: () => host.querySelector(".conflict"),
  };
}

/** Let the save's promise chain run out, then render. */
async function settle() {
  for (let i = 0; i < 6; i++) await Promise.resolve();
  flushSync();
}

const FILE = 'title = "before"\n';
const EDIT = 'title = "after"\n';

describe("TextTab, a save the core refuses", () => {
  it("keeps the draft on screen", async () => {
    // ⛔ `dirty = false` ran synchronously on the click, so the re-seed effect
    // put the on-disk text back BEFORE the failure arrived: the edit was gone
    // and the error named a line the user could no longer see.
    const h = render(FILE, BAD);
    h.type("title = ");
    await h.save();
    expect(h.saved).toEqual(["title = "]);
    expect(h.box().value).toBe("title = ");
  });

  it("still takes a later reload once the edit has been saved", async () => {
    const h = render(FILE, OK);
    h.type(EDIT);
    await h.save();
    expect(h.box().value).toBe(EDIT);
    // The file moves on (another window saved a form field); the buffer is no
    // longer dirty, so it follows.
    h.props.text = 'title = "elsewhere"\n';
    flushSync();
    expect(h.box().value).toBe('title = "elsewhere"\n');
  });

  it("leaves a typed draft alone when the file changes underneath it", () => {
    const h = render(FILE, OK);
    h.type(EDIT);
    h.props.text = 'title = "elsewhere"\n';
    flushSync();
    expect(h.box().value).toBe(EDIT);
  });
});

describe("TextTab, a file that moved underneath the buffer", () => {
  it("asks before overwriting an edit made outside this window", async () => {
    // ⛔ `save_config_text` had no compare-and-swap and no watcher event
    // reached this window, so a buffer seeded an hour ago silently reverted
    // whatever `$EDITOR` had written since. The shell now refuses with
    // `conflict`, and the tab has to turn that into a question.
    const h = render(FILE, OK, () => 'title = "from $EDITOR"\n');
    h.type(EDIT);
    await h.save();
    expect(h.saved).toEqual([]);
    expect(h.conflict()).toBeTruthy();
    expect(h.box().value).toBe(EDIT);
  });

  it("writes when told to overwrite, and not before", async () => {
    const h = render(FILE, OK, () => 'title = "from $EDITOR"\n');
    h.type(EDIT);
    await h.save();
    await h.overwrite();
    expect(h.saved).toEqual([EDIT]);
    expect(h.conflict()).toBe(null);
  });

  it("stops and says so when the file cannot be re-read for an overwrite", async () => {
    // ⛔ A failed re-read fell back to the stale base, so the save conflicted
    // again and every click showed the same banner with no reason at all.
    const h = render(FILE, OK, () => 'title = "from $EDITOR"\n');
    h.type(EDIT);
    await h.save();
    h.props.diskText = () => Promise.reject("permission denied: bridgewatch.toml");
    flushSync();
    await h.overwrite();
    expect(h.saved).toEqual([]);
    expect(host.querySelector(".read-error")?.textContent).toContain(
      "Could not re-read the file: permission denied: bridgewatch.toml",
    );
    expect(h.box().value).toBe(EDIT);
  });

  it("offers to take the file instead, which is a revert", async () => {
    const h = render(FILE, OK, () => 'title = "from $EDITOR"\n');
    h.type(EDIT);
    await h.save();
    host.querySelectorAll<HTMLButtonElement>(".conflict button")[1].click();
    flushSync();
    expect(h.reverts.length).toBe(1);
    expect(h.saved).toEqual([]);
    expect(h.conflict()).toBe(null);
  });

  it("saves straight through when nothing else has touched the file", async () => {
    const h = render(FILE, OK);
    h.type(EDIT);
    await h.save();
    expect(h.saved).toEqual([EDIT]);
    expect(h.conflict()).toBe(null);
  });
});

describe("TextTab's other buttons and its diagnostics list", () => {
  function mountTab(props: Partial<ComponentProps<typeof TextTab>> = {}) {
    host = document.createElement("div");
    document.body.append(host);
    const calls = { validated: [] as string[], saves: 0, reverts: 0 };
    const all = reactive({
      text: "a = 1\n",
      validation: null,
      diskText: () => Promise.resolve("a = 1\n"),
      onvalidate: (t: string) => void calls.validated.push(t),
      onsave: () => {
        calls.saves++;
        return new Promise<Validation>(() => {});
      },
      onrevert: () => void calls.reverts++,
      ...props,
    }) as ComponentProps<typeof TextTab>;
    component = mount(TextTab, { target: host, props: all });
    flushSync();
    return { calls, props: all };
  }

  const button = (name: string) => [...host.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === name)!;

  it("validates the box's text, not the file's", () => {
    const { calls } = mountTab();
    const box = host.querySelector("textarea")!;
    box.value = "a = 2\n";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    button("Validate").click();
    expect(calls.validated).toEqual(["a = 2\n"]);
  });

  it("sends one save however fast Save is clicked twice", () => {
    const { calls } = mountTab();
    button("Save").click();
    button("Save").click();
    expect(calls.saves).toBe(1);
  });

  it("puts the file's text back on Revert and asks the window to re-read it", () => {
    const { calls, props } = mountTab();
    const box = host.querySelector("textarea")!;
    box.value = "a = 3\n";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    button("Revert").click();
    flushSync();
    expect(calls.reverts).toBe(1);
    // No longer dirty: the next text from disk is shown as is.
    props.text = "b = 1\n";
    flushSync();
    expect(host.querySelector("textarea")!.value).toBe("b = 1\n");
  });

  it("lists warnings in amber, and a diagnostic with no position by its path alone", () => {
    mountTab({
      validation: {
        ok: true,
        diagnostics: [{ severity: "warning", path: "watches[0]", message: "slow poll", line: null, col: null }],
      },
    });
    const item = host.querySelector(".diagnostics li")!;
    expect(item.className).toContain("text-tone-amber");
    expect(item.querySelector(".where")?.textContent?.trim()).toBe("watches[0]");
  });
});
