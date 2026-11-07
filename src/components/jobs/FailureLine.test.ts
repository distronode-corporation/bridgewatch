import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { JobActionOutcome, LogTail } from "../../lib/types";
import { job } from "./__fixtures__/pipeline";
import JobList from "./JobList.svelte";
import { canCopyFailure, codeSpan, failureMarkdown, type JobTools } from "./tools";

/**
 * F2: a failed job's first failing line, under its row, and "Copy failure as
 * Markdown". Nothing here reaches Tauri: the clipboard is `tools.api.copy`.
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

const FAILED = job({
  id: 16558670141,
  name: "verify:android",
  stage: "verify",
  class: "blocking_failure",
  status: "failed",
  web_url: "https://gitlab.com/acme-corp/monorepo/-/jobs/16558670141",
  failure: "script failure",
});
const PASSED = job({ id: 2, name: "build", stage: "build" });

function tools(copy: (text: string) => Promise<boolean>): JobTools {
  return {
    watchId: "main-push",
    provider: "gitlab",
    actions: false,
    context: "pipeline #13074",
    api: {
      logTail: vi.fn(() => new Promise<LogTail>(() => {})),
      action: vi.fn(() => new Promise<JobActionOutcome>(() => {})),
      copy,
    },
  };
}

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

const $ = (selector: string) => host.querySelector<HTMLElement>(selector);
const $$ = (selector: string) => [...host.querySelectorAll<HTMLElement>(selector)];

function mountList(jobs = [PASSED, FAILED], t?: JobTools) {
  component = mount(JobList, { target: host, props: { jobs, onOpen: vi.fn(), now: 0, tools: t } });
  flushSync();
}

describe("the first failing line", () => {
  it("is drawn under a failed job, and only under a failed job", () => {
    const stale = job({ id: 3, name: "e2e", status: "success", failure: "left over" });
    mountList([PASSED, FAILED, stale]);
    const lines = $$('[data-slot="job-failure"]');
    expect(lines.map((l) => l.dataset.jobId)).toEqual([String(FAILED.id)]);
    const text = lines[0].querySelector<HTMLElement>('[data-slot="job-failure-text"]')!;
    expect(text.textContent?.replace(/\s+/g, " ").trim()).toBe(
      "first failing line of verify:android: script failure",
    );
    expect(text.title).toBe("script failure");
    // Directly under its own job's row.
    const row = $(`li[data-job-id="${FAILED.id}"]:not([data-slot])`);
    expect(row?.nextElementSibling).toBe(lines[0]);
  });

  it("needs no tools, and a failed job nobody said why about has no line", () => {
    mountList([job({ ...FAILED, failure: null }), job({ ...FAILED, id: 9, name: "lint" })]);
    expect($$('[data-slot="job-failure"]').map((l) => l.dataset.jobId)).toEqual(["9"]);
    expect($('[data-slot="job-copy"]')).toBeNull();
  });
});

describe("Copy failure as Markdown", () => {
  it("says what failed and where, the line in a fence, and the link", () => {
    expect(failureMarkdown(FAILED, "pipeline #13074")).toBe(
      "`verify:android` failed in acme-corp/monorepo, pipeline #13074.\n\n" +
        "```\nscript failure\n```\n\n" +
        "https://gitlab.com/acme-corp/monorepo/-/jobs/16558670141\n",
    );
  });

  it("survives backticks in the line and in the name, and leaves out what it lacks", () => {
    const github = job({
      id: 9102,
      name: "test `unit`",
      status: "failed",
      web_url: "https://github.com/acme-corp/monorepo/actions/runs/4101/job/9102",
      failure: "src/parser.rs:142: assertion ```left == right``` failed",
    });
    expect(failureMarkdown(github, "workflow run #77")).toBe(
      "`` test `unit` `` failed in acme-corp/monorepo, workflow run #77.\n\n" +
        "````\nsrc/parser.rs:142: assertion ```left == right``` failed\n````\n\n" +
        "https://github.com/acme-corp/monorepo/actions/runs/4101/job/9102\n",
    );
    expect(failureMarkdown(job({ id: 1, name: "lint", status: "failed", web_url: null }), "")).toBe(
      "`lint` failed.\n",
    );
    expect(codeSpan("`a`")).toBe("`` `a` ``");
  });

  it("is offered on a failed job only", () => {
    expect(canCopyFailure(FAILED)).toBe(true);
    expect(canCopyFailure(job({ ...FAILED, failure: null }))).toBe(true);
    expect(canCopyFailure(PASSED)).toBe(false);
    expect(canCopyFailure(job({ id: 4, name: "x", status: "canceled" }))).toBe(false);
  });

  it("puts the Markdown on the clipboard and says so, then goes back to copy", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const copy = vi.fn(async () => true);
    mountList([PASSED, FAILED], tools(copy));
    const buttons = $$('[data-slot="job-copy"]');
    expect(buttons).toHaveLength(1);
    const button = buttons[0];
    expect(button.getAttribute("aria-label")).toBe("Copy the failure of verify:android as Markdown");
    // At the end of the failing line, not among the row's tools: the line
    // truncates rather than the job's name.
    expect(button.closest('[data-slot="job-failure"]')).not.toBeNull();
    expect(button.closest('[data-slot="job-tools"]')).toBeNull();

    button.click();
    await vi.advanceTimersByTimeAsync(0);
    flushSync();
    expect(copy).toHaveBeenCalledWith(failureMarkdown(FAILED, "pipeline #13074"));
    expect(button.textContent?.trim()).toBe("copied");
    expect($('[data-slot="job-copy-status"]')?.textContent).toBe("Copied the failure of verify:android");

    await vi.advanceTimersByTimeAsync(1500);
    flushSync();
    expect(button.textContent?.trim()).toBe("copy");
    expect($('[data-slot="job-copy-status"]')?.textContent).toBe("");
  });

  it("goes among the row's tools, always shown, on a failed job with no line", () => {
    mountList([job({ ...FAILED, failure: null })], tools(async () => true));
    const button = $('[data-slot="job-copy"]')!;
    expect(button.closest('[data-slot="job-tools"]')?.hasAttribute("data-always")).toBe(true);
    expect($('[data-slot="job-failure"]')).toBeNull();
  });

  it("says so when the clipboard refused", async () => {
    mountList([FAILED], tools(async () => false));
    $('[data-slot="job-copy"]')!.click();
    await settle();
    expect($('[data-slot="job-copy"]')?.textContent?.trim()).toBe("not copied");
    expect($('[data-slot="job-copy-status"]')?.textContent).toBe("Could not copy to the clipboard");
  });
});
