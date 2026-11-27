// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { describe, expect, it, vi } from "vitest";

import { projectOf, SHELL_API } from "./tools";

const invoked: { cmd: string; args: unknown }[] = [];

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    invoked.push({ cmd, args });
    return Promise.resolve(null);
  },
}));

describe("SHELL_API", () => {
  it("sends the log tail and the job action to the shell, and copies through the clipboard", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const url = "https://gitlab.com/acme/web/-/jobs/7";
    await SHELL_API.logTail("main-push", url);
    await SHELL_API.action("main-push", url, "play");
    await expect(SHELL_API.copy("failed: lint")).resolves.toBe(true);
    expect(invoked).toEqual([
      { cmd: "job_log_tail", args: { watch: "main-push", url } },
      { cmd: "job_action", args: { watch: "main-push", url, action: "play" } },
    ]);
    expect(writeText).toHaveBeenCalledWith("failed: lint");
    delete (navigator as unknown as Record<string, unknown>).clipboard;
  });
});

describe("projectOf on URLs that name no project", () => {
  it("returns null for a job URL with an empty project path, or a page that is neither provider's job", () => {
    expect(projectOf("https://gitlab.com/-/jobs/1")).toBeNull();
    expect(projectOf("https://example.com/acme/web/pipelines/1")).toBeNull();
    expect(projectOf(null)).toBeNull();
  });
});
