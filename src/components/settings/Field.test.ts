import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import { entry } from "../../lib/settings/registry";
import type { Edit } from "../../lib/types";
import Field from "./Field.svelte";

/**
 * The generic settings control.
 *
 * Every tab but "Edit as text" is built out of this component, so the two
 * things it gets wrong are wrong everywhere: what an EMPTIED field writes, and
 * what the control shows after the core refuses the value in it.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

interface Harness {
  props: { entry: ReturnType<typeof entry>; path: string; value: unknown; onedit: (e: Edit[]) => Promise<void> };
  edits: Edit[][];
  /** Resolve the pending edit with the value the file now holds. */
  answer: (value?: unknown) => Promise<void>;
  input: () => HTMLInputElement;
  textarea: () => HTMLTextAreaElement;
}

function render(path: string, value: unknown): Harness {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  let release: (() => void) | null = null;
  const props = reactive({
    entry: entry(path),
    path: path.replace(/\*/g, "0"),
    value,
    onedit: (e: Edit[]) =>
      new Promise<void>((resolve) => {
        edits.push(e);
        release = resolve;
      }),
  }) as Harness["props"];
  component = mount(Field, { target: host, props });
  return {
    props,
    edits,
    // The core answers, the document is re-read, and only then does the
    // component get to react — which is the ordering the real `edit()` has.
    answer: async (next?: unknown) => {
      if (next !== undefined) props.value = next;
      flushSync();
      release?.();
      await Promise.resolve();
      await Promise.resolve();
      flushSync();
    },
    input: () => host.querySelector("input")!,
    textarea: () => host.querySelector("textarea")!,
  };
}

function change(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  element.value = value;
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

describe("Field, what an emptied control writes", () => {
  it("removes a key whose default is absence", () => {
    // `dive.only_when` is an Option in the core: absent means "always dive".
    const h = render("watches.*.dive.only_when", "failed");
    change(h.input(), "");
    expect(h.edits).toEqual([[{ op: "unset", path: "watches.0.dive.only_when" }]]);
  });

  it("writes an empty STRING where the default is not absence", () => {
    // ⛔ `dive.bridges` defaults to `"*"`. Unsetting it is not "dive into
    // nothing", it is "dive into every bridge of every pipeline" — the exact
    // opposite of what the field's own hint says an empty value means.
    const h = render("watches.*.dive.bridges", "trigger:website");
    change(h.input(), "");
    expect(h.edits).toEqual([
      [{ op: "set", path: "watches.0.dive.bridges", value: { string: "" } }],
    ]);
  });

  it("marks that key, and only that key, in the registry", () => {
    expect(entry("watches.*.dive.bridges").emptyMeans).toBe("empty");
    expect(entry("watches.*.dive.only_when").emptyMeans).toBe(undefined);
  });
});

describe("Field, after the core has answered", () => {
  it("does not keep a value the file refused", async () => {
    // The core rejects the save, so `value` never changes. Nothing re-renders,
    // and the control would go on showing a number the file does not contain.
    const h = render("watches.*.show.max_rows", 20);
    change(h.input(), "999");
    await h.answer();
    expect(h.input().value).toBe("20");
  });

  it("shows the accepted value when the file did change", async () => {
    const h = render("watches.*.show.max_rows", 20);
    change(h.input(), "5");
    expect(h.edits).toEqual([[{ op: "set", path: "watches.0.show.max_rows", value: { integer: 5 } }]]);
    await h.answer(5);
    expect(h.input().value).toBe("5");
  });

  it("writes nothing for a blank or unparseable number", async () => {
    const h = render("watches.*.poll.live_secs", 5);
    change(h.input(), "");
    expect(h.edits).toEqual([]);
    expect(h.input().value).toBe("5");
    change(h.input(), "soon");
    expect(h.edits).toEqual([]);
    expect(h.input().value).toBe("5");
  });

  it("removes an optional number whose entry says absence is its default", () => {
    // `fan_out_secs` is absent-means-90; every other number keeps its value.
    expect(entry("watches.*.fan_out_secs").emptyMeans).toBe("unset");
    const h = render("watches.*.fan_out_secs", 120);
    change(h.input(), "");
    expect(h.edits).toEqual([[{ op: "unset", path: "watches.0.fan_out_secs" }]]);
  });

  it("restores a list textarea the core would not take", async () => {
    const h = render("watches.*.deploy_markers", ["deploy:origins"]);
    change(h.textarea(), "deploy:origins\ndeploy:cdn_seo");
    expect(h.edits).toEqual([
      [
        {
          op: "set",
          path: "watches.0.deploy_markers",
          value: { array: [{ string: "deploy:origins" }, { string: "deploy:cdn_seo" }] },
        },
      ],
    ]);
    await h.answer();
    expect(h.textarea().value).toBe("deploy:origins");
  });
});

describe("the job-list controls", () => {
  it("a watch's override offers 'inherit', and choosing it removes the key", async () => {
    const h = render("watches.*.show.jobs", undefined);
    const select = host.querySelector("select") as HTMLSelectElement;
    expect([...select.options].map((o) => o.value)).toEqual(["", "failures", "all"]);
    expect(select.value).toBe("");

    select.value = "failures";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    expect(h.edits[0]).toEqual([
      { op: "set", path: "watches.0.show.jobs", value: { string: "failures" } },
    ]);
    await h.answer("failures");

    select.value = "";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    expect(h.edits[1]).toEqual([{ op: "unset", path: "watches.0.show.jobs" }]);
  });

  it("the global setting is failures or all", () => {
    render("ui.jobs", "all");
    const select = host.querySelector("select") as HTMLSelectElement;
    expect([...select.options].map((o) => o.value)).toEqual(["failures", "all"]);
    expect(select.value).toBe("all");
  });
});
