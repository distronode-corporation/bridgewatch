import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { JobAction, JobActionOutcome, LogTail } from "../../lib/types";
import { PARENT_JOBS, job } from "./__fixtures__/pipeline";
import JobList from "./JobList.svelte";
import { canPlay, canRetry, canShowLog, confirmText, createSingleFlight, projectOf, type JobTools } from "./tools";

/**
 * The job view's log, retry and play tools, against a scripted shell. Nothing
 * here reaches Tauri: every command goes through `tools.api`.
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
});

/** A promise and the two hands that settle it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tools(overrides: Partial<JobTools> = {}): JobTools {
  return {
    watchId: "main-push",
    provider: "gitlab",
    actions: true,
    context: "pipeline #42",
    api: {
      logTail: vi.fn(() => new Promise<LogTail>(() => {})),
      action: vi.fn(() => new Promise<JobActionOutcome>(() => {})),
    },
    ...overrides,
  };
}

/** Let every pending promise callback run, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

const $ = (selector: string) => host.querySelector<HTMLElement>(selector);
const click = (el: HTMLElement | null) => {
  el!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  flushSync();
};

function mountList(t: JobTools, jobs = PARENT_JOBS) {
  component = mount(JobList, { target: host, props: { jobs, onOpen: vi.fn(), now: 0, tools: t } });
  flushSync();
}

describe("which jobs get which tools", () => {
  it("a failed job gets a log; retry needs actions; play is a GitLab manual job's", () => {
    const failed = job({ id: 1, name: "lint", status: "failed" });
    const canceled = job({ id: 2, name: "e2e", status: "canceled" });
    const manual = job({ id: 3, name: "deploy", status: "manual" });
    const on = tools();
    expect(canShowLog(failed)).toBe(true);
    expect(canShowLog(canceled)).toBe(false);
    expect(canRetry(failed, on) && canRetry(canceled, on)).toBe(true);
    expect(canRetry(failed, tools({ actions: false }))).toBe(false);
    expect(canPlay(manual, on)).toBe(true);
    expect(canPlay(manual, tools({ provider: "github" }))).toBe(false);
    expect(canPlay(manual, tools({ actions: false }))).toBe(false);
  });

  it("without tools the list draws no buttons at all", () => {
    component = mount(JobList, { target: host, props: { jobs: PARENT_JOBS, onOpen: vi.fn(), now: 0 } });
    flushSync();
    expect($('[data-slot="job-log-toggle"]')).toBeNull();
    expect($('[data-slot="job-retry"]')).toBeNull();
  });

  it("without actions a failed job offers its log and no retry", () => {
    mountList(tools({ actions: false }));
    expect(host.querySelectorAll('[data-slot="job-log-toggle"]')).toHaveLength(2);
    expect($('[data-slot="job-retry"]')).toBeNull();
  });
});

describe("the log", () => {
  it("loads on open, shows loading, then the lines as text with a link out", async () => {
    const tail = deferred<LogTail>();
    const t = tools();
    t.api!.logTail = vi.fn(() => tail.promise);
    const onOpen = vi.fn();
    component = mount(JobList, { target: host, props: { jobs: PARENT_JOBS, onOpen, now: 0, tools: t } });
    flushSync();

    expect(t.api!.logTail).not.toHaveBeenCalled();
    const toggle = $('[data-job-id="101"] [data-slot="job-log-toggle"]')!;
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(t.api!.logTail).toHaveBeenCalledWith("main-push", "https://gitlab.example/p/-/jobs/101");
    expect($('[data-slot="job-log-loading"]')?.getAttribute("role")).toBe("status");

    tail.resolve({ lines: ["$ make lint", "<img src=x onerror=alert(1)>"], truncated: true });
    await settle();
    const pre = $('[data-slot="job-log-text"]')!;
    expect(pre.tagName).toBe("PRE");
    expect(pre.textContent).toBe("$ make lint\n<img src=x onerror=alert(1)>");
    expect(pre.querySelector("img"), "a log line was rendered as markup").toBeNull();
    expect(pre.getAttribute("tabindex")).toBe("0");
    expect($('[data-slot="job-log-truncated"]')).not.toBeNull();

    click($('[data-slot="job-log-open"]'));
    expect(onOpen).toHaveBeenCalledWith("https://gitlab.example/p/-/jobs/101");
  });

  it("shows an error inline, as an alert", async () => {
    const t = tools();
    t.api!.logTail = vi.fn(() => Promise.reject("not found: /projects/1/jobs/101/trace"));
    mountList(t);
    click($('[data-job-id="101"] [data-slot="job-log-toggle"]'));
    await settle();
    const error = $('[data-slot="job-log-error"]')!;
    expect(error.getAttribute("role")).toBe("alert");
    expect(error.textContent).toContain("not found");
  });

  it("says so when the log is empty", async () => {
    const t = tools();
    t.api!.logTail = vi.fn(() => Promise.resolve({ lines: [], truncated: false }));
    mountList(t);
    click($('[data-job-id="101"] [data-slot="job-log-toggle"]'));
    await settle();
    expect($('[data-slot="job-log-empty"]')).not.toBeNull();
  });
});

describe("retry and play", () => {
  it("asks first, naming the job, the project and the pipeline, and Cancel sends nothing", () => {
    const t = tools();
    mountList(t);
    click($('[data-job-id="101"] [data-slot="job-retry"]'));
    expect($('[data-slot="job-confirm-text"]')?.textContent).toBe('Retry job "lint" in p, pipeline #42?');
    click($('[data-slot="job-confirm-no"]'));
    expect($('[data-slot="job-confirm"]')).toBeNull();
    expect(t.api!.action).not.toHaveBeenCalled();
  });

  it("a double click on the confirm sends exactly one request", async () => {
    const sent = deferred<JobActionOutcome>();
    const t = tools();
    t.api!.action = vi.fn(() => sent.promise);
    mountList(t);
    click($('[data-job-id="101"] [data-slot="job-retry"]'));
    const yes = $('[data-slot="job-confirm-yes"]')!;
    // Both clicks land before anything re-renders, which is the case a
    // disabled attribute alone cannot stop.
    yes.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    yes.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    flushSync();
    click(yes);
    expect(t.api!.action).toHaveBeenCalledTimes(1);
    expect(t.api!.action).toHaveBeenCalledWith("main-push", "https://gitlab.example/p/-/jobs/101", "retry");

    sent.resolve({ job_id: 8, web_url: null });
    await settle();
    expect($('[data-slot="job-action-done"]')?.textContent).toContain("Retry sent.");
  });

  it("shows a refusal inline and lets the person try again", async () => {
    const t = tools();
    const outcomes: Array<() => Promise<JobActionOutcome>> = [
      () => Promise.reject("this token cannot run jobs (403): it needs the api scope"),
      () => Promise.resolve({ job_id: 9, web_url: null }),
    ];
    t.api!.action = vi.fn((_w: string, _u: string, _a: JobAction) => outcomes.shift()!());
    const manual = job({ id: 300, name: "deploy:prod", stage: "deploy", status: "manual", class: "gate" });
    mountList(t, [manual]);
    click($('[data-job-id="300"] [data-slot="job-play"]'));
    expect($('[data-slot="job-confirm-text"]')?.textContent).toBe('Start manual job "deploy:prod" in p, pipeline #42?');
    click($('[data-slot="job-confirm-yes"]'));
    await settle();
    expect($('[data-slot="job-action-error"]')?.textContent).toContain("api scope");
    click($('[data-slot="job-confirm-yes"]'));
    await settle();
    expect(t.api!.action).toHaveBeenCalledTimes(2);
    expect($('[data-slot="job-action-done"]')?.textContent).toContain("Started.");
  });

  it("the buttons are keyboard buttons that say what they control", () => {
    mountList(tools());
    const retry = $('[data-job-id="101"] [data-slot="job-retry"]') as HTMLButtonElement;
    expect(retry.tagName).toBe("BUTTON");
    expect(retry.getAttribute("aria-label")).toBe("Retry lint");
    click(retry);
    expect(retry.getAttribute("aria-expanded")).toBe("true");
    expect(document.getElementById(retry.getAttribute("aria-controls")!)).not.toBeNull();
  });
});

describe("helpers", () => {
  it("reads the project out of either provider's job URL, for display", () => {
    expect(projectOf("https://gitlab.com/acme-corp/platform/monorepo/-/jobs/1")).toBe("acme-corp/platform/monorepo");
    expect(projectOf("https://github.com/acme-corp/monorepo/actions/runs/1/job/2")).toBe("acme-corp/monorepo");
    expect(projectOf("not a url")).toBeNull();
    expect(confirmText("retry", job({ id: 1, name: "x", web_url: null }), "pipeline #1")).toBe('Retry job "x" in pipeline #1?');
  });

  it("single-flight frees its key when the task settles, even on failure", async () => {
    const flight = createSingleFlight();
    const first = flight.run("k", () => Promise.reject(new Error("no")));
    expect(flight.run("k", () => Promise.resolve(1))).toBeNull();
    await expect(first).rejects.toThrow("no");
    expect(flight.busy("k")).toBe(false);
    await expect(flight.run("k", () => Promise.resolve(2))).resolves.toBe(2);
  });
});
