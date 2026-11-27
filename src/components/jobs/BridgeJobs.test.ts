// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { bridge, job } from "./__fixtures__/pipeline";
import BridgeJobs from "./BridgeJobs.svelte";
import { createExpansionStore } from "./jobs";
import type { JobTools } from "./tools";

/** One bridge on its own: the dead-reason words, the links a workflow run has, and the confirm text's place name. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(props: Partial<ComponentProps<typeof BridgeJobs>> & Pick<ComponentProps<typeof BridgeJobs>, "bridge">) {
  host = document.createElement("div");
  document.body.append(host);
  const onOpen = vi.fn();
  component = mount(BridgeJobs, {
    target: host,
    props: { pipelineId: 42, expansion: createExpansionStore(), onOpen, defaultOpen: true, ...props },
  });
  flushSync();
  return { root: host, onOpen };
}

const tools: JobTools = {
  watchId: "main-push",
  provider: "gitlab",
  actions: true,
  context: "pipeline #42",
  api: { logTail: () => new Promise(() => {}), action: () => new Promise(() => {}) },
};

const failedJob = job({ id: 7, name: "lint", status: "failed", class: "blocking_failure" });

function confirmTextAfterRetry(root: HTMLElement): string | null | undefined {
  root.querySelector<HTMLElement>('[data-slot="job-retry"]')!.click();
  flushSync();
  return root.querySelector('[data-slot="job-confirm-text"]')?.textContent;
}

describe("BridgeJobs", () => {
  it("gives a dead bridge that has a child no reason of its own", () => {
    const { root } = render({ bridge: bridge({ name: "trigger:web", verdict: "dead", status: "failed" }) });
    expect(root.querySelector('[aria-label^="trigger:web:"]')?.getAttribute("aria-label")).toBe("trigger:web: dead. Hide jobs");
    expect(root.querySelector('[data-slot="bridge-no-child"]')).toBeNull();
  });

  it("says a dived bridge with no child pipeline has none", () => {
    const { root } = render({ bridge: bridge({ name: "trigger:web", child_id: null, child_url: null }) });
    expect(root.textContent).toContain("No child pipeline.");
  });

  it("names the child pipeline by its id and its bridge in the confirm text", () => {
    const { root } = render({ bridge: bridge({ name: "trigger:web", child_id: 901, jobs: [failedJob] }), tools });
    expect(confirmTextAfterRetry(root)).toBe('Retry job "lint" in p, child pipeline #901 of trigger:web?');
  });

  it("names a workflow run by its name in the confirm text", () => {
    const { root } = render({ bridge: bridge({ name: "CI", jobs: [failedJob] }), provider: "github", tools: { ...tools, provider: "github" } });
    expect(confirmTextAfterRetry(root)).toMatch(/in .*workflow run CI\?$/);
  });

  it("opens a workflow run from its child URL when it has no page of its own, and shows no link with neither", () => {
    const run = bridge({ name: "CI", web_url: null, child_url: "https://github.com/acme/web/actions/runs/9" });
    const { root, onOpen } = render({ bridge: run, provider: "github" });
    const link = root.querySelector<HTMLAnchorElement>('[data-slot="bridge-run-link"]')!;
    expect(link.getAttribute("href")).toBe("https://github.com/acme/web/actions/runs/9");
    link.click();
    expect(onOpen).toHaveBeenCalledWith("https://github.com/acme/web/actions/runs/9");
    void unmount(component!);
    host!.remove();
    const { root: bare } = render({ bridge: { ...run, child_url: null }, provider: "github" });
    expect(bare.querySelector('[data-slot="bridge-run-link"]')).toBeNull();
  });
});
