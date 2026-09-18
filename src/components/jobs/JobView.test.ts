import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { BridgeView, JobsMode, PipelineView } from "../../lib/types";
import { PARENT_JOBS, RUNNING_JOB, T0, bridge, pipeline } from "./__fixtures__/pipeline";
import BridgeJobs from "./BridgeJobs.svelte";
import JobList from "./JobList.svelte";
import PipelineJobs from "./PipelineJobs.svelte";
import UpdatedAgo from "./UpdatedAgo.svelte";
import { createExpansionStore, type ExpansionStore } from "./jobs";

/**
 * The job view mounted against fixtures. Re-renders go through `reactive()`
 * props: a refresh that only exists on the SECOND render is exactly what the
 * expansion test is about.
 */

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host.remove();
  vi.useRealTimers();
});

const stages = () => [...host.querySelectorAll<HTMLElement>("[data-stage]")].map((s) => s.dataset.stage);
const jobNames = () => [...host.querySelectorAll<HTMLElement>("[data-job-id] a, [data-job-id] > span.font-mono")].map(
  (a) => a.textContent?.trim(),
);
const bridgeOpen = (name: string) =>
  host.querySelector<HTMLElement>(`[data-bridge="${name}"] [data-slot="collapsible-trigger"]`)?.getAttribute(
    "aria-expanded",
  );

describe("JobList", () => {
  it("groups by stage in pipeline order with a dot per job", () => {
    component = mount(JobList, { target: host, props: { jobs: PARENT_JOBS, onOpen: () => {}, now: T0 } });
    flushSync();
    expect(stages()).toEqual(["build", "test", "deploy"]);
    expect(host.querySelectorAll('[data-slot="job-dot"]')).toHaveLength(5);
    const lint = host.querySelector('[data-job-id="101"] [data-slot="job-dot"]')!;
    expect(lint.className).toContain("bg-tone-red");
  });

  it("in failures mode lists only the failures", () => {
    component = mount(JobList, {
      target: host,
      props: { jobs: PARENT_JOBS, mode: "failures", onOpen: () => {}, now: T0 },
    });
    flushSync();
    expect(jobNames()).toEqual(["lint", "e2e"]);
    expect(stages()).toEqual(["build", "test"]);
  });

  it("says so when failures mode leaves nothing", () => {
    component = mount(JobList, {
      target: host,
      props: { jobs: PARENT_JOBS.filter((j) => j.class === "passed"), mode: "failures", onOpen: () => {} },
    });
    flushSync();
    expect(host.querySelector('[data-slot="job-list-empty"]')?.textContent).toBe("No failures.");
  });

  it("hands the job URL to onOpen and never navigates", () => {
    const onOpen = vi.fn();
    component = mount(JobList, { target: host, props: { jobs: PARENT_JOBS, onOpen, now: T0 } });
    flushSync();
    const link = host.querySelector<HTMLAnchorElement>('[data-job-id="101"] a')!;
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(event);
    expect(onOpen).toHaveBeenCalledWith("https://gitlab.example/p/-/jobs/101");
    expect(event.defaultPrevented).toBe(true);
  });

  it("marks a running job live and ticks its duration every second", () => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    component = mount(JobList, { target: host, props: { jobs: [RUNNING_JOB, ...PARENT_JOBS], onOpen: () => {} } });
    flushSync();
    const row = () => host.querySelector('[data-job-id="203"]')!;
    const duration = () => row().querySelector('[data-slot="job-duration"]')?.textContent?.trim();
    expect(row().querySelector('[data-slot="live-marker"]')).not.toBeNull();
    expect(row().querySelector('[data-slot="job-dot"]')!.className).toContain("bw-live");
    expect(duration()).toBe("1m 05s");

    vi.advanceTimersByTime(1_000);
    flushSync();
    expect(duration()).toBe("1m 06s");

    vi.advanceTimersByTime(5_000);
    flushSync();
    expect(duration()).toBe("1m 11s");

    // A finished job does not tick.
    expect(host.querySelector('[data-job-id="102"] [data-slot="job-duration"]')?.textContent?.trim()).toBe("3m 07s");
  });

  it("runs no timer when nothing is running", () => {
    vi.useFakeTimers();
    component = mount(JobList, { target: host, props: { jobs: PARENT_JOBS, onOpen: () => {} } });
    flushSync();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("PipelineJobs", () => {
  function render(initial: { pipeline: PipelineView; mode?: JobsMode; expansion?: ExpansionStore }) {
    const props = reactive({
      pipeline: initial.pipeline,
      mode: initial.mode ?? ("all" as JobsMode),
      expansion: initial.expansion ?? createExpansionStore(),
      onOpen: () => {},
      now: T0,
    });
    component = mount(PipelineJobs, { target: host, props });
    flushSync();
    return props;
  }

  it("shows parent jobs and every bridge, open while the pipeline is live", () => {
    render({ pipeline: pipeline() });
    expect(host.querySelectorAll("[data-bridge]")).toHaveLength(3);
    expect(bridgeOpen("trigger:app")).toBe("true");
    // The child's stages, in order, inside the open bridge.
    const app = host.querySelector('[data-bridge="trigger:app"]')!;
    expect([...app.querySelectorAll<HTMLElement>("[data-stage]")].map((s) => s.dataset.stage)).toEqual([
      "prepare",
      "build",
      "publish",
    ]);
  });

  it("starts a settled pipeline's bridges collapsed", () => {
    render({ pipeline: pipeline({ live: false }) });
    expect(bridgeOpen("trigger:app")).toBe("false");
  });

  it("keeps what the user opened and closed across a snapshot refresh", () => {
    const props = render({ pipeline: pipeline({ live: false }) });
    const trigger = (name: string) =>
      host.querySelector<HTMLButtonElement>(`[data-bridge="${name}"] [data-slot="collapsible-trigger"]`)!;

    trigger("trigger:docs").click();
    flushSync();
    expect(bridgeOpen("trigger:docs")).toBe("true");

    // A poll replaces every object, reorders the bridges and flips the
    // pipeline to live (which would change the default for untouched bridges).
    const next = structuredClone(pipeline({ live: false }));
    next.bridges.reverse();
    props.pipeline = next;
    flushSync();
    expect(bridgeOpen("trigger:docs")).toBe("true");
    expect(bridgeOpen("trigger:app")).toBe("false");

    // And a choice to CLOSE survives a pipeline going live.
    trigger("trigger:docs").click();
    flushSync();
    props.pipeline = structuredClone(pipeline({ live: true }));
    flushSync();
    expect(bridgeOpen("trigger:docs")).toBe("false");
    expect(bridgeOpen("trigger:app")).toBe("true");
  });

  it("keeps expansion when the whole component is remounted with the same store", () => {
    const expansion = createExpansionStore();
    render({ pipeline: pipeline({ live: false }), expansion });
    host.querySelector<HTMLButtonElement>('[data-bridge="trigger:android"] [data-slot="collapsible-trigger"]')!.click();
    flushSync();
    void unmount(component!);
    render({ pipeline: structuredClone(pipeline({ live: false })), expansion });
    expect(bridgeOpen("trigger:android")).toBe("true");
  });

  it("in failures mode drops quiet bridges and passing parent jobs", () => {
    const props = render({ pipeline: pipeline(), mode: "failures" });
    // trigger:app has no failing job and is running; trigger:docs passed.
    expect([...host.querySelectorAll<HTMLElement>("[data-bridge]")].map((b) => b.dataset.bridge)).toEqual([
      "trigger:android",
    ]);
    expect(host.textContent).not.toContain("build:web");
    expect(host.textContent).toContain("lint");
    const android = host.querySelector('[data-bridge="trigger:android"]')!;
    expect(android.textContent).toContain("android:test");
    expect(android.textContent).not.toContain("android:build");

    props.mode = "all";
    flushSync();
    expect(host.querySelectorAll("[data-bridge]")).toHaveLength(3);
    expect(host.textContent).toContain("build:web");
  });
});

describe("BridgeJobs, the two links on the header", () => {
  function render(view: BridgeView, onOpen: (url: string) => void = () => {}) {
    component = mount(BridgeJobs, {
      target: host,
      props: { bridge: view, pipelineId: 5000, expansion: createExpansionStore(), onOpen, now: T0 },
    });
    flushSync();
  }

  const link = (slot: string) => host.querySelector<HTMLAnchorElement>(`[data-slot="${slot}"]`);

  it("opens the trigger job itself, not the child pipeline", () => {
    const onOpen = vi.fn();
    render(bridge({ name: "trigger:website", web_url: "https://gitlab.example/p/-/jobs/77" }), onOpen);
    const job = link("bridge-job-link")!;
    // The accessible name has to say what the link opens: "job" beside "child"
    // means nothing read on its own.
    expect(job.getAttribute("aria-label")).toBe("Open trigger job trigger:website in GitLab");
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    job.dispatchEvent(event);
    expect(onOpen.mock.calls).toEqual([["https://gitlab.example/p/-/jobs/77"]]);
    // Never navigates: a GitLab page inside the popover is a trap.
    expect(event.defaultPrevented).toBe(true);
  });

  it("renders no trigger-job link when the bridge has no web_url", () => {
    render(bridge({ name: "trigger:website", web_url: null }));
    expect(link("bridge-job-link")).toBeNull();
    // The name is still there, as the plain text it already was.
    expect(host.textContent).toContain("trigger:website");
  });

  it("keeps the trigger job and the child pipeline on separate links", () => {
    const onOpen = vi.fn();
    render(
      bridge({
        name: "trigger:website",
        web_url: "https://gitlab.example/p/-/jobs/77",
        child_url: "https://gitlab.example/p/-/pipelines/900",
      }),
      onOpen,
    );
    link("bridge-child-link")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    link("bridge-job-link")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    // ⛔ By value and in order: the two URLs are interchangeable to the type
    // checker, so only the arguments themselves prove they were not swapped.
    expect(onOpen.mock.calls).toEqual([
      ["https://gitlab.example/p/-/pipelines/900"],
      ["https://gitlab.example/p/-/jobs/77"],
    ]);
  });

  it("clicking a link neither opens nor closes the row", () => {
    const expansion = createExpansionStore();
    component = mount(BridgeJobs, {
      target: host,
      props: { bridge: bridge({ name: "trigger:website" }), pipelineId: 5000, expansion, onOpen: () => {}, now: T0 },
    });
    flushSync();
    expect(bridgeOpen("trigger:website")).toBe("false");
    link("bridge-job-link")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    flushSync();
    expect(bridgeOpen("trigger:website")).toBe("false");
  });
});

describe("UpdatedAgo", () => {
  it("ticks once a second", () => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    component = mount(UpdatedAgo, { target: host, props: { at: new Date(T0 - 2_000).toISOString() } });
    flushSync();
    expect(host.textContent?.trim()).toBe("updated 2s ago");
    vi.advanceTimersByTime(3_000);
    flushSync();
    expect(host.textContent?.trim()).toBe("updated 5s ago");
  });
});
