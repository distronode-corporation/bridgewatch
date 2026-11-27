// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import fixture from "../lib/__fixtures__/ca41ab28.json" with { type: "json" };
import type { RequestLog, Snapshot } from "../lib/types";
import DebugPane from "./DebugPane.svelte";

const snapshot = fixture as unknown as Snapshot;

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

function render(props: Record<string, unknown>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(DebugPane, { target: host, props: { snapshot, status: null, open: true, ...props } });
  flushSync();
  return host;
}

function stubClipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
}

beforeEach(() => vi.useFakeTimers());

afterEach(() => {
  if (component) void unmount(component);
  host?.remove();
  component = null;
  host = null;
  delete (navigator as unknown as Record<string, unknown>).clipboard;
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const copyButton = (root: HTMLElement) =>
  [...root.querySelectorAll("button")].find((b) => /cop/i.test(b.textContent ?? ""))!;

describe("DebugPane's copy button", () => {
  it("copies the snapshot as JSON, says so, and goes back to its label after two seconds", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    stubClipboard(writeText);
    const root = render({});
    copyButton(root).click();
    await vi.advanceTimersByTimeAsync(0);
    flushSync();
    expect(JSON.parse(writeText.mock.calls[0][0] as string)).toEqual(snapshot);
    expect(copyButton(root).textContent?.trim()).toBe("Copied");
    await vi.advanceTimersByTimeAsync(2000);
    flushSync();
    expect(copyButton(root).textContent?.trim()).toBe("Copy verdict JSON");
  });

  it("says when the clipboard refused", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stubClipboard(() => Promise.reject(new Error("not focused")));
    const root = render({});
    copyButton(root).click();
    await vi.advanceTimersByTimeAsync(0);
    flushSync();
    expect(copyButton(root).textContent?.trim()).toBe("Copy failed");
  });
});

describe("DebugPane's request ring", () => {
  const entry = (partial: Partial<RequestLog>): RequestLog => ({
    method: "GET",
    path: "/projects/1/pipelines",
    status: 200,
    ms: 120,
    ratelimit_remaining: 1999,
    ratelimit_reset: null,
    retry_after: null,
    error: null,
    at: "2026-09-18T10:00:00Z",
    ...partial,
  });

  it("shows a transport error where the status would be, and a dash when there is neither", () => {
    const log = [
      entry({ status: null, error: "connection reset", ratelimit_remaining: null, retry_after: 30 }),
      entry({ status: null, error: null }),
    ];
    const rows = [...render({ snapshot: { ...snapshot, request_log: log } }).querySelectorAll("tbody tr")];
    const cells = rows.map((r) => [...r.querySelectorAll("td")].map((td) => td.textContent));
    expect(cells[0][2]).toBe("connection reset");
    expect(cells[0][4]).toBe("");
    expect(cells[0][5]).toBe("30");
    expect(cells[1][2]).toBe("—");
  });

  it("shows the last poll's age and the config path from the status", () => {
    const status = { configPath: "/x/bridgewatch.toml", sinceLastPollSecs: 4, nextPollSecs: 6 };
    const text = render({ status }).textContent ?? "";
    expect(text).toContain("/x/bridgewatch.toml");
    expect(text).toContain("4s ago");
  });

  it("says never before the first poll", () => {
    const status = { configPath: "/x/bridgewatch.toml", sinceLastPollSecs: null, nextPollSecs: null };
    expect(render({ status }).textContent).toContain("never");
  });

  it("says how to turn the ring on when it holds nothing", () => {
    const root = render({ snapshot: { ...snapshot, request_log: [] } });
    expect(root.querySelector("table")).toBeNull();
    expect(root.textContent).toContain("No requests recorded.");
  });
});
