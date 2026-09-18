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
