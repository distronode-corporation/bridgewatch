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
  { id: "main-push", role: "primary", account: "gitlab.com", project: 12345678, ref: "main" },
  { id: "hourly", role: "secondary", account: "gitlab.com", project: 12345678, ref: "main" },
  { id: "preflights", role: "secondary", account: "gitlab.com", project: 12345678, ref: "pf/*" },
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

describe("WatchesTab, keys for the other provider", () => {
  function renderMixed() {
    host = document.createElement("div");
    document.body.append(host);
    const props = reactive({
      config: {
        accounts: { lab: { provider: "gitlab" }, hub: { provider: "github" } },
        watches: [
          { id: "lab-main", account: "lab", project: 1, ref: "main", workflow: "ci.yml" },
          { id: "hub-main", account: "hub", project: "acme/web", ref: "main", workflow: "ci.yml" },
        ],
      },
      jobOrder: [[], []],
      onedit: () => Promise.resolve(),
      onremove: () => {},
      onmove: () => {},
      onadd: () => {},
      }) as ComponentProps<typeof WatchesTab>;
    component = mount(WatchesTab, { target: host, props });
    flushSync();
  }

  /** The field row whose label reads `label`, in the open panel. */
  const row = (label: string) =>
    [...host.querySelectorAll<HTMLLabelElement>("section.watch .body label.field")].find(
      (l) => l.querySelector(".label")?.textContent === label,
    )!;

  it("dims workflow on a GitLab watch and says why, but leaves it editable with its value", () => {
    renderMixed();
    const workflow = row("Workflow");
    expect(workflow.dataset.inapplicable).toBe("true");
    expect(workflow.querySelector('[data-slot="provider-note"]')?.textContent).toMatch(/GitLab project has no workflows/);
    const input = workflow.querySelector<HTMLInputElement>("input")!;
    // ⛔ Not hidden and not disabled: the value came with the watch and has to
    // be visible to be cleared.
    expect(input.disabled).toBe(false);
    expect(input.value).toBe("ci.yml");
    // So are the commit group's two keys.
    for (const label of ["One row is", "Commit window (s)"]) {
      expect(row(label).dataset.inapplicable, label).toBe("true");
      expect(row(label).querySelector("input, select, button")?.hasAttribute("disabled"), label).toBe(false);
    }
    // A dive key is GitLab's and is left alone here.
    expect(row("Dive depth").dataset.inapplicable).toBeUndefined();
  });

  it("dims dive depth on a GitHub watch, and not its workflow, its group or its dive selection", () => {
    renderMixed();
    host.querySelectorAll<HTMLButtonElement>("header button.disclosure")[1].click();
    flushSync();
    for (const label of ["Workflow", "One row is", "Commit window (s)"]) {
      expect(row(label).dataset.inapplicable, label).toBeUndefined();
    }
    expect(row("Dive depth").dataset.inapplicable).toBe("true");
    expect(row("Dive depth").querySelector("input")?.hasAttribute("disabled")).toBe(false);
    // On a commit group each run is a bridge, so these select workflow names.
    for (const label of ["Dive into bridges", "Dive exclusions", "Dive only when"]) {
      expect(row(label).dataset.inapplicable, label).toBeUndefined();
    }
  });
});

describe("WatchesTab, groups (L2)", () => {
  const groups = () => [...host.querySelectorAll<HTMLDetailsElement>("section.watch .body details.settings-group")];

  it("splits the open watch into seven groups by config path, only Source open", () => {
    render();
    expect(groups().map((g) => g.dataset.group)).toEqual([
      "Source",
      "Rows",
      "Polling",
      "Child pipelines",
      "Deploy",
      "Jobs",
      "Notifications",
    ]);
    expect(groups().map((g) => g.open)).toEqual([true, false, false, false, false, false, false]);
    const labels = (name: string) =>
      [...groups().find((g) => g.dataset.group === name)!.querySelectorAll(".label")].map((l) => l.textContent);
    expect(labels("Source")).toContain("Project");
    expect(labels("Polling")).toEqual(["Poll while live (s)", "Poll while idle (s)"]);
    expect(labels("Jobs")).toContain("Job class overrides");
    expect(labels("Notifications")).toContain("Fixed again");
    expect(labels("Notifications")).toContain("Click opens");
    // The heading says "Notifications", so no label repeats it.
    expect(labels("Notifications").some((l) => /^Notif/.test(l ?? ""))).toBe(false);
  });

  it("keeps a group the user opened open across a reload of the file", () => {
    const h = render();
    const polling = () => groups().find((g) => g.dataset.group === "Polling")!;
    polling().open = true;
    polling().dispatchEvent(new Event("toggle"));
    flushSync();
    h.reload(WATCHES.map((w) => ({ ...w })));
    expect(polling().open).toBe(true);
  });

  it("says so when there are no watches", () => {
    render([]);
    expect(host.querySelector(".empty")?.textContent).toContain("No watches yet");
  });
});

describe("WatchesTab, a long account name (cubic, #65)", () => {
  it("sizes the account menu to its names, capped at the row, with the full name as its title", () => {
    const long = "gitlab.engineering-division.example.com-platform-team";
    host = document.createElement("div");
    document.body.append(host);
    const props = reactive({
      config: { accounts: { [long]: {} }, watches: [{ id: "w", account: long, project: 1, ref: "main" }] },
      jobOrder: [[]],
      onedit: () => Promise.resolve(),
      onremove: () => {},
      onmove: () => {},
      onadd: () => {},
    }) as ComponentProps<typeof WatchesTab>;
    component = mount(WatchesTab, { target: host, props });
    flushSync();
    const select = [...host.querySelectorAll<HTMLLabelElement>("label.field")]
      .find((l) => l.querySelector(".label")?.textContent === "Account")!
      .querySelector("select")!;
    const cls = select.getAttribute("class") ?? "";
    // Not a fixed width: as wide as the longest name, at least 12rem, at most the row.
    expect(cls).not.toMatch(/(^|\s)w-48(\s|$)/);
    expect(cls).toMatch(/(^|\s)w-auto(\s|$)/);
    expect(cls).toMatch(/(^|\s)min-w-48(\s|$)/);
    expect(cls).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(select.title).toBe(long);
  });
});
