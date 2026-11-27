// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { job } from "./__fixtures__/pipeline";
import JobList from "./JobList.svelte";
import type { JobTools } from "./tools";

/** The job list on its own: a job with no page, and copying a failure with the shell's clipboard. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
  delete (navigator as unknown as Record<string, unknown>).clipboard;
});

function render(props: Record<string, unknown>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(JobList, { target: host, props: { onOpen: vi.fn(), now: 0, ...props } as never });
  flushSync();
  return host;
}

describe("JobList", () => {
  it("names a job with no page as plain text, not a link", () => {
    const root = render({ jobs: [job({ id: 1, name: "orphan", web_url: null })] });
    const name = [...root.querySelectorAll("span")].find((s) => s.textContent === "orphan");
    expect(name?.getAttribute("title")).toBe("orphan");
    expect(root.querySelector("a")).toBeNull();
  });

  it("copies a failure with no failing line from its row, through the shell's clipboard when the tools bring none", async () => {
    const writeText = vi.fn((_text: string) => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const tools: JobTools = {
      watchId: "main-push",
      provider: "gitlab",
      actions: false,
      context: "pipeline #42",
      api: { logTail: () => new Promise(() => {}), action: () => new Promise(() => {}) },
    };
    const failed = job({ id: 3, name: "lint", status: "failed", class: "blocking_failure" });
    const root = render({ jobs: [failed], tools });
    root.querySelector<HTMLButtonElement>('[data-slot="job-copy"]')!.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText.mock.calls[0][0]).toContain("lint");
    expect(root.querySelector('[data-slot="job-copy"]')?.textContent).toBe("copied");
  });
});
