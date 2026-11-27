// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import UpdatedAgo from "./UpdatedAgo.svelte";

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

function render(props: Record<string, unknown>): HTMLElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(UpdatedAgo, { target: host, props });
  flushSync();
  return host.querySelector<HTMLElement>('[data-slot="updated-ago"]')!;
}

afterEach(() => {
  if (component) void unmount(component);
  host?.remove();
  component = null;
  host = null;
});

describe("UpdatedAgo's title", () => {
  it("shows an epoch-milliseconds poll time as an ISO instant", () => {
    const at = Date.parse("2026-09-18T10:00:00Z");
    expect(render({ at, now: at + 4000 }).title).toBe("2026-09-18T10:00:00.000Z");
  });

  it("has no title before the first poll", () => {
    expect(render({ at: null, now: 0 }).title).toBe("");
  });
});
