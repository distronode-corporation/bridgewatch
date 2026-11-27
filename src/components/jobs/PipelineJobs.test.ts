// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { bridge, job, pipeline } from "./__fixtures__/pipeline";
import { createExpansionStore } from "./jobs";
import PipelineJobs from "./PipelineJobs.svelte";
import type { JobTools } from "./tools";

/** A pipeline's job view on its own: nothing to list, and the confirm text's name for the run. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(props: Record<string, unknown>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(PipelineJobs, { target: host, props: { expansion: createExpansionStore(), onOpen: vi.fn(), ...props } as never });
  flushSync();
  return host;
}

const failed = job({ id: 7, name: "lint", status: "failed", class: "blocking_failure" });
const tools: JobTools = {
  watchId: "main-push",
  provider: "gitlab",
  actions: true,
  context: "unused",
  api: { logTail: () => new Promise(() => {}), action: () => new Promise(() => {}) },
};

function confirmText(root: HTMLElement) {
  root.querySelector<HTMLElement>('[data-slot="job-retry"]')!.click();
  flushSync();
  return root.querySelector('[data-slot="job-confirm-text"]')?.textContent;
}

describe("PipelineJobs", () => {
  it("says there are no jobs, or no failures, when nothing is left to list", () => {
    const empty = pipeline({ parent_jobs: [], bridges: [] });
    expect(render({ pipeline: empty }).querySelector('[data-slot="pipeline-jobs-empty"]')?.textContent?.trim()).toBe("No jobs.");
    void unmount(component!);
    host!.remove();
    const allGreen = pipeline({ parent_jobs: [job({ id: 1, name: "unit" })], bridges: [bridge({ name: "trigger:web", verdict: "passed" })] });
    expect(render({ pipeline: allGreen, mode: "failures" }).querySelector('[data-slot="pipeline-jobs-empty"]')?.textContent?.trim()).toBe(
      "No failures.",
    );
  });

  it("lists parent jobs with no bridge section when there are no bridges", () => {
    const root = render({ pipeline: pipeline({ parent_jobs: [failed], bridges: [] }) });
    expect(root.querySelector('[role="list"]')).toBeNull();
    expect(root.textContent).toContain("lint");
  });

  it("names a workflow run by its run number, and a pipeline with no iid by its id", () => {
    const run = pipeline({ id: 555, iid: 12, parent_jobs: [failed], bridges: [] });
    expect(confirmText(render({ pipeline: run, provider: "github", tools: { ...tools, provider: "github" } }))).toMatch(/in .*workflow run #12\?$/);
    void unmount(component!);
    host!.remove();
    const noIid = pipeline({ id: 555, iid: null, parent_jobs: [failed], bridges: [] });
    expect(confirmText(render({ pipeline: noIid, tools }))).toMatch(/in .*pipeline #555\?$/);
  });
});
