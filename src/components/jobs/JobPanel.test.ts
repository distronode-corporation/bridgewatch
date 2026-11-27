// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { JobView, LogTail } from "../../lib/types";
import { job } from "./__fixtures__/pipeline";
import JobPanel from "./JobPanel.svelte";
import type { JobTools } from "./tools";

/** The panel under one job, mounted on its own: the log's lifetime, and a job that has no page. */

const invoked: { cmd: string; args: unknown }[] = [];

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    invoked.push({ cmd, args });
    return Promise.resolve({ lines: ["from the shell"], truncated: false });
  },
}));

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
  invoked.length = 0;
});

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

function render(props: { job: JobView; tools: JobTools; log: boolean; confirm: "retry" | "play" | null }) {
  const onOpen = vi.fn();
  const reactiveProps = reactive({ ...props, onCancel: vi.fn(), onOpen, id: "panel" });
  host = document.createElement("div");
  document.body.append(host);
  component = mount(JobPanel, { target: host, props: reactiveProps });
  flushSync();
  return { root: host, props: reactiveProps, onOpen };
}

const failed = job({ id: 7, name: "lint", status: "failed", class: "blocking_failure" });
const base: JobTools = { watchId: "main-push", provider: "gitlab", actions: true, context: "pipeline #42" };

describe("JobPanel", () => {
  it("reads the log through the shell when the tools bring no API of their own", async () => {
    const { root } = render({ job: failed, tools: base, log: true, confirm: null });
    await settle();
    expect(invoked).toEqual([{ cmd: "job_log_tail", args: { watch: "main-push", url: failed.web_url } }]);
    expect(root.querySelector('[data-slot="job-log-text"]')?.textContent).toBe("from the shell");
  });

  it("drops a log that arrives after the section closed, and an error the same way", async () => {
    let resolveTail!: (tail: LogTail) => void;
    let rejectTail!: (error: unknown) => void;
    const logTail = vi
      .fn(() => new Promise<LogTail>(() => {}))
      .mockImplementationOnce(() => new Promise<LogTail>((resolve) => (resolveTail = resolve)))
      .mockImplementationOnce(() => new Promise<LogTail>((_, reject) => (rejectTail = reject)));
    const tools = { ...base, api: { logTail, action: vi.fn() } };
    const { root, props } = render({ job: failed, tools, log: true, confirm: null });
    props.log = false;
    flushSync();
    resolveTail({ lines: ["late"], truncated: false });
    await settle();
    props.log = true;
    flushSync();
    expect(root.querySelector('[data-slot="job-log-loading"]')).not.toBeNull();
    props.log = false;
    flushSync();
    rejectTail(new Error("too late"));
    await settle();
    props.log = true;
    flushSync();
    // A third open fetches again; neither late answer was shown in between.
    expect(logTail).toHaveBeenCalledTimes(3);
    expect(root.querySelector('[data-slot="job-log-text"]')).toBeNull();
    expect(root.querySelector('[data-slot="job-log-error"]')).toBeNull();
  });

  it("sends nothing, links nothing and fetches nothing for a job with no page", async () => {
    const action = vi.fn();
    const logTail = vi.fn();
    const bare = { ...failed, web_url: null };
    const { root } = render({ job: bare, tools: { ...base, api: { logTail, action } }, log: true, confirm: "retry" });
    root.querySelector<HTMLButtonElement>('[data-slot="job-confirm-yes"]')!.click();
    await settle();
    expect(action).not.toHaveBeenCalled();
    expect(logTail).not.toHaveBeenCalled();
    expect(root.querySelector('[data-slot="job-log-open"]')).toBeNull();
  });
});
