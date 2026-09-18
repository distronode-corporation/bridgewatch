import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { Edit } from "../../lib/types";
import WatchesTab from "./WatchesTab.svelte";

/**
 * The watch list.
 *
 * ⚠ The defect here is a SECOND-render one: the open panel was remembered by
 * position, and only a move or a removal renumbers the list. A test that
 * mounts once and reads the DOM cannot see it.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

const WATCHES = [
  { id: "main-push", role: "primary", account: "gitlab.com", project: 82468124, ref: "main" },
  { id: "hourly", role: "secondary", account: "gitlab.com", project: 82468124, ref: "main" },
  { id: "preflights", role: "secondary", account: "gitlab.com", project: 82468124, ref: "pf/*" },
];

function render(watches = WATCHES) {
  host = document.createElement("div");
  document.body.append(host);
  const edits: Edit[][] = [];
  const props = reactive({
    config: { accounts: { "gitlab.com": {} }, watches },
    jobOrder: watches.map(() => []),
    onedit: (e: Edit[]) => {
      edits.push(e);
      return Promise.resolve();
    },
    onremove: () => {},
    onmove: () => {},
    onadd: () => {},
  }) as ComponentProps<typeof WatchesTab>;
  component = mount(WatchesTab, { target: host, props });
  flushSync();
  return {
    props,
    edits,
    /** What the parent does after a move or a removal: it re-reads the file. */
    reload: (next: typeof WATCHES) => {
      props.config = { accounts: { "gitlab.com": {} }, watches: next };
      flushSync();
    },
    headers: () => [...host.querySelectorAll("section.watch header strong")].map((s) => s.textContent),
    /** The id of the watch whose form is on screen, read off its own fields. */
    openWatch: () => {
      const body = host.querySelector("section.watch .body");
      if (!body) return null;
      const section = body.closest("section.watch")!;
      return section.querySelector("header strong")?.textContent ?? null;
    },
    toggle: (index: number) => {
      host.querySelectorAll<HTMLButtonElement>("header button.disclosure")[index].click();
      flushSync();
    },
  };
}

describe("WatchesTab", () => {
  it("opens the first watch and only that one", () => {
    const h = render();
    expect(h.headers()).toEqual(["main-push", "hourly", "preflights"]);
    expect(h.openWatch()).toBe("main-push");
    expect(host.querySelectorAll("section.watch .body").length).toBe(1);
  });

  it("keeps the panel on the WATCH when a move renumbers the list", () => {
    // ⛔ The open panel was an index. "Move up" renumbers, so the form stayed
    // at position 1 while the watch it was showing moved to position 0: the
    // fields on screen silently became a different watch's, and the next thing
    // typed went into it.
    const h = render();
    h.toggle(1);
    expect(h.openWatch()).toBe("hourly");
    h.reload([WATCHES[1], WATCHES[0], WATCHES[2]]);
    expect(h.headers()).toEqual(["hourly", "main-push", "preflights"]);
    expect(h.openWatch()).toBe("hourly");
  });

  it("closes rather than following the position when the open watch is removed", () => {
    const h = render();
    h.toggle(2);
    expect(h.openWatch()).toBe("preflights");
    h.reload([WATCHES[0], WATCHES[1]]);
    expect(h.openWatch()).toBe(null);
  });

  it("closes the panel it has open when its own disclosure is clicked again", () => {
    const h = render();
    h.toggle(0);
    expect(h.openWatch()).toBe(null);
    h.toggle(0);
    expect(h.openWatch()).toBe("main-push");
  });
});
