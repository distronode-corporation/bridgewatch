import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import { entry } from "../../lib/settings/registry";
import type { Edit } from "../../lib/types";
import KeyValueField from "./KeyValueField.svelte";

/**
 * The `[watches.jobs]` editor: an ordered table, where order is the rule.
 *
 * Both defects held here are invisible on the first render. The uncommitted
 * row exists only in the component, and the value a rejected save leaves
 * behind only appears once the document has come back UNCHANGED — which is
 * exactly the render a test with a plain props object never performs.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

const JOBS = entry("watches.*.jobs.*");

function render(rows: [string, string][]) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const props = reactive({
    entry: JOBS,
    path: "watches.0.jobs",
    rows,
    values: ["gate", "warning", "blocking", "ignore"],
    keyPlaceholder: "job name or re:pattern",
    onedit: (e: Edit[]) => {
      edits.push(e);
      return Promise.resolve();
    },
  }) as ComponentProps<typeof KeyValueField>;
  component = mount(KeyValueField, { target: host, props });
  flushSync();
  return {
    edits,
    /** What the parent does after ANY save: it re-reads the document. */
    reload: async (next?: [string, string][]) => {
      props.rows = (next ?? props.rows).map((r: [string, string]) => [r[0], r[1]] as [string, string]);
      await Promise.resolve();
      await Promise.resolve();
      flushSync();
    },
    keys: () => [...host.querySelectorAll<HTMLInputElement>(".row input[type=text]")],
    picks: () => [...host.querySelectorAll<HTMLSelectElement>(".row select")],
    add: () => {
      host.querySelector<HTMLButtonElement>("button.add")!.click();
      flushSync();
    },
    removeAt: (index: number) => {
      host.querySelectorAll<HTMLButtonElement>('.row button[title="remove"]')[index].click();
      flushSync();
    },
    moveUp: (index: number) => {
      host.querySelectorAll<HTMLButtonElement>('.row button[title="move up"]')[index].click();
      flushSync();
    },
  };
}

function change(element: HTMLInputElement | HTMLSelectElement, value: string) {
  element.value = value;
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

describe("KeyValueField", () => {
  const rows: [string, string][] = [
    ["verify:*", "warning"],
    ["deploy:origins", "blocking"],
  ];

  it("renders the rows in file order, which is the precedence order", () => {
    const h = render(rows);
    expect(h.keys().map((i) => i.value)).toEqual(["verify:*", "deploy:origins"]);
    expect(h.picks().map((s) => s.value)).toEqual(["warning", "blocking"]);
  });

  it("keeps an uncommitted row across a save somewhere else", async () => {
    // ⛔ "Add row" writes nothing: a blank key is not a row the file can hold.
    // So the row lives in the component alone, and a reload caused by any other
    // field's save used to delete it mid-sentence.
    const h = render(rows);
    h.add();
    expect(h.keys().length).toBe(3);
    await h.reload();
    expect(h.keys().length).toBe(3);
    expect(h.keys()[2].value).toBe("");
    expect(h.edits).toEqual([]);
  });

  it("writes the new row once it has a key, and does not then duplicate it", async () => {
    const h = render(rows);
    h.add();
    change(h.keys()[2], "kics-iac-sast");
    expect(h.edits).toEqual([
      [{ op: "set", path: 'watches.0.jobs."kics-iac-sast"', value: { string: "gate" } }],
    ]);
    // The document now holds it, so the re-seed must not leave the pending copy
    // behind as well.
    await h.reload([...rows, ["kics-iac-sast", "gate"]]);
    expect(h.keys().map((i) => i.value)).toEqual(["verify:*", "deploy:origins", "kics-iac-sast"]);
  });

  it("changes one value with one edit rather than rewriting the table", async () => {
    // The rewrite loses the table's comments; the review's example loses two.
    const h = render(rows);
    change(h.picks()[0], "ignore");
    expect(h.edits).toEqual([
      [{ op: "set", path: 'watches.0.jobs."verify:*"', value: { string: "ignore" } }],
    ]);
  });

  it("does not keep a value the core refused", async () => {
    const h = render(rows);
    change(h.keys()[0], "verify:everything");
    change(h.picks()[0], "ignore");
    // The save failed, so the document comes back saying what it said before.
    // ⚠ The KEY is the one that proves it: Svelte re-asserts a `<select>` on
    // every render whether or not its value expression changed, so the pick
    // corrects itself, while the text input keeps whatever was typed into it
    // until something writes the document's value back.
    await h.reload();
    expect(h.keys()[0].value).toBe("verify:*");
    expect(h.picks()[0].value).toBe("warning");
  });

  it("removes a row by unsetting its key, and drops an uncommitted one with no save", () => {
    const h = render(rows);
    h.removeAt(1);
    expect(h.edits).toEqual([[{ op: "unset", path: 'watches.0.jobs."deploy:origins"' }]]);
    h.add();
    h.removeAt(2);
    expect(h.edits.length).toBe(1);
    expect(h.keys().length).toBe(2);
  });

  it("rewrites the whole table for a move, because order is the rule", () => {
    const h = render(rows);
    h.moveUp(1);
    expect(h.edits).toEqual([
      [
        { op: "unset", path: "watches.0.jobs" },
        { op: "set", path: 'watches.0.jobs."deploy:origins"', value: { string: "blocking" } },
        { op: "set", path: 'watches.0.jobs."verify:*"', value: { string: "warning" } },
      ],
    ]);
  });
});
