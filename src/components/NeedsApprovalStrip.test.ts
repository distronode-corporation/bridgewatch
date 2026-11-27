// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import NeedsApprovalStrip from "./NeedsApprovalStrip.svelte";

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

function render(props: Record<string, unknown>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(NeedsApprovalStrip, { target: host, props });
  flushSync();
  return host;
}

afterEach(() => {
  if (component) void unmount(component);
  host?.remove();
  component = null;
  host = null;
});

describe("NeedsApprovalStrip", () => {
  it("renders nothing without a list of accounts", () => {
    expect(render({}).querySelector('[data-slot="needs-approval"]')).toBeNull();
  });

  it("names several accounts in the plural", () => {
    const strip = render({ accounts: ["gl", "gh"] }).querySelector('[data-slot="needs-approval"]');
    expect(strip?.textContent).toContain("Accounts gl, gh: the credential store's prompt was cancelled");
  });

  it("offers no Retry when there is nobody to ask", () => {
    const strip = render({ accounts: ["gl"] }).querySelector('[data-slot="needs-approval"]');
    expect(strip).not.toBeNull();
    expect(strip?.querySelector('[data-action="approval-retry"]')).toBeNull();
  });
});
