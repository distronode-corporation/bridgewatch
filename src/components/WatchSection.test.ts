import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import fixture from "../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { PipelineView, Snapshot, WatchView } from "../lib/types";
import { createExpansionStore } from "./jobs";
import WatchSection from "./WatchSection.svelte";

/**
 * One watch's block in the popover, mounted on its own.
 *
 * The popover test covers what this renders from a real snapshot; what is here
 * is the part that needs a CLICK (a secondary watch is one line until asked)
 * and the names the core reports as failures elsewhere in the pipeline, which
 * are links or they are dead text.
 */

const snapshot = fixture as unknown as Snapshot;
const ROW = snapshot.watches[0].rows[0];

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
});

function render(watch: WatchView) {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(WatchSection, {
    target: host,
    props: { watch, now: Date.parse("2026-09-17T15:00:00Z"), expansion: createExpansionStore() },
  });
  flushSync();
  return {
    rows: () => host.querySelectorAll(".row"),
    disclose: () => {
      host.querySelector<HTMLButtonElement>("button.disclosure")!.click();
      flushSync();
    },
  };
}

function secondary(rows: PipelineView[]): WatchView {
  return { id: "hourly", role: "secondary", icon_state: null, rows, error: null };
}

describe("WatchSection, a secondary watch", () => {
  it("shows its newest row and nothing else until it is opened", () => {
    // ⚠ Not zero rows: a header with no information in it reads as a bug. The
    // point of the role is that an hourly schedule which is red by design does
    // not shout, not that it disappears.
    const h = render(secondary([ROW, { ...ROW, id: ROW.id - 1, sha7: "0000000" }]));
    expect(h.rows().length).toBe(1);
    expect(host.querySelector(".sha")?.textContent?.trim()).toBe("ca41ab2");
    h.disclose();
    expect(h.rows().length).toBe(2);
  });

  it("keeps the collapsed row's detail hidden", () => {
    const h = render(secondary([ROW]));
    expect(host.querySelectorAll("[data-bridge]").length).toBe(0);
    expect(host.querySelector('[data-slot="pipeline-jobs"]')).toBe(null);
    h.disclose();
    expect(host.querySelectorAll("[data-bridge]").length).toBe(ROW.bridges.length);
  });
});

describe("WatchSection, a primary watch with nothing in it", () => {
  it("says why it is empty and does not call itself secondary", () => {
    render({ id: "main-push", role: "primary", icon_state: null, rows: [], error: null });
    expect(host.querySelector(".role")).toBe(null);
    expect(host.textContent).toContain("no matching pipelines");
    expect(host.querySelector("button.disclosure")).toBe(null);
  });

  it("shows the error instead, when there is one", () => {
    render({
      id: "main-push",
      role: "primary",
      icon_state: null,
      rows: [],
      error: "401 Unauthorized (token for account gitlab.com)",
    });
    expect(host.querySelector(".error")?.textContent).toContain("401 Unauthorized");
    expect(host.textContent).not.toContain("no matching pipelines");
  });
});

describe("PipelineRow, the names of failures that are somewhere else", () => {
  it("links a post-deploy failure to the job inside the child pipeline", () => {
    // ⛔ `parent_jobs` alone misses both kinds: a sibling failure is usually a
    // BRIDGE, and a post-deploy failure happened inside a CHILD. Every such
    // name rendered as plain text on a row otherwise full of working links,
    // which is the one name on the row that explains the icon.
    const bridge = ROW.bridges.find((b) => b.jobs.length > 0)!;
    const job = bridge.jobs[0];
    render({
      id: "main-push",
      role: "primary",
      icon_state: "deployed_with_failure",
      rows: [{ ...ROW, sibling_failures: [], post_deploy_failures: [job.name] }],
      error: null,
    });
    const note = [...host.querySelectorAll(".notes .note")].find((n) =>
      n.textContent?.includes("after deploy"),
    );
    expect(note, "no post-deploy note on the row").toBeTruthy();
    expect(note!.querySelector("a")?.getAttribute("href")).toBe(job.web_url);
  });

  it("renders a name it cannot find as text rather than a dead link", () => {
    render({
      id: "main-push",
      role: "primary",
      icon_state: "deployed_with_failure",
      rows: [{ ...ROW, sibling_failures: ["a-job-in-a-pipeline-nobody-fetched"] }],
      error: null,
    });
    const note = [...host.querySelectorAll(".notes .note")].find((n) =>
      n.textContent?.includes("sibling"),
    );
    expect(note?.textContent).toContain("a-job-in-a-pipeline-nobody-fetched");
    expect(note?.querySelector("a[href]")).toBe(null);
  });
});

describe("WatchSection, the job view's mode", () => {
  const primary = (jobs?: "failures" | "all"): WatchView => ({
    id: "main-push",
    role: "primary",
    icon_state: "deployed_with_failure",
    rows: [ROW],
    error: null,
    ...(jobs ? { jobs } : {}),
  });

  it("follows the effective mode the core resolved for the watch", () => {
    render(primary("failures"));
    expect(host.querySelector('[data-slot="pipeline-jobs"]')?.getAttribute("data-mode")).toBe("failures");
    // Failures mode drops the passing parent jobs the "all" view lists.
    const passing = ROW.parent_jobs.find((j) => j.class === "passed");
    expect(passing, "the fixture has a passing parent job").toBeTruthy();
    expect(host.querySelector(`[data-job-id="${passing!.id}"]`)).toBe(null);
  });

  it("lists every job when the watch says all, and when an old recording says nothing", () => {
    const passing = ROW.parent_jobs.find((j) => j.class === "passed")!;
    render(primary("all"));
    expect(host.querySelector('[data-slot="pipeline-jobs"]')?.getAttribute("data-mode")).toBe("all");
    expect(host.querySelector(`[data-job-id="${passing.id}"]`)).toBeTruthy();
    void unmount(component!);
    component = null;
    host.remove();
    render(primary());
    expect(host.querySelector('[data-slot="pipeline-jobs"]')?.getAttribute("data-mode")).toBe("all");
  });
});
