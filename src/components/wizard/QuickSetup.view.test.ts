// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { RecentProject } from "./api";
import QuickSetup from "./QuickSetup.svelte";

/** The one-click section on its own: a listing that failed, one the server would not give, and GitHub's words. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(props: Partial<ComponentProps<typeof QuickSetup>>): HTMLDivElement {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(QuickSetup, {
    target: host,
    props: {
      provider: "gitlab",
      listing: null,
      loading: false,
      loadError: null,
      ticks: {},
      onToggle: vi.fn(),
      onSetUp: vi.fn(),
      busy: false,
      now: Date.parse("2026-11-26T12:00:00Z"),
      ...props,
    },
  });
  flushSync();
  return host;
}

const text = (selector: string) => host!.querySelector(selector)?.textContent?.replace(/\s+/g, " ").trim();

const repo: RecentProject = {
  id: 1,
  path: "acme/web",
  name: "web",
  default_branch: "main",
  last_activity_at: "2026-11-26T11:00:00Z",
  web_url: "https://github.com/acme/web",
  ticked: true,
};

describe("QuickSetup", () => {
  it("draws nothing with no listing, no error and nothing loading", () => {
    expect(render({}).querySelector('[data-slot="quick-setup"]')).toBeNull();
  });

  it("says why the recent projects could not be listed", () => {
    render({ loadError: "403 Forbidden." });
    expect(text('[data-slot="quick-error"]')).toBe(
      "Could not list your recent projects: 403 Forbidden. Choose Next to pick one instead.",
    );
  });

  it("gives the server's reason when it lists none", () => {
    render({ listing: { mode: "unlisted", reason: "This token cannot list projects." } });
    expect(text('[data-slot="quick-unlisted"]')).toBe("This token cannot list projects.");
  });

  it("counts one ticked repository in the singular", () => {
    render({ provider: "github", listing: { mode: "projects", projects: [repo], listed_by: { provider: "github", origin: "https://api.github.com" } }, ticks: { 1: true } });
    expect(text('[data-action="quick-setup"]')).toBe("Watch 1 repository");
  });
});
