// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { ProjectListing } from "./api";
import { emptyDraft } from "./model";
import ProjectStep from "./ProjectStep.svelte";

/** The project step on its own: a listing that failed or was refused, the search box, and the typed project. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

const LISTING: ProjectListing = {
  mode: "projects",
  projects: [{ id: 1, path: "acme/web", name: "web", default_branch: null, web_url: null }],
  truncated: true,
};

function render(props: Partial<ComponentProps<typeof ProjectStep>> = {}) {
  const calls = { search: [] as string[], resolve: 0 };
  const all = reactive({
    draft: emptyDraft(),
    errors: {},
    listing: LISTING,
    loading: false,
    loadError: null,
    resolving: false,
    onPick: vi.fn(),
    onResolve: () => void calls.resolve++,
    onSearch: (term: string) => void calls.search.push(term),
    ...props,
  }) as ComponentProps<typeof ProjectStep>;
  host = document.createElement("div");
  document.body.append(host);
  component = mount(ProjectStep, { target: host, props: all });
  flushSync();
  return { props: all, calls };
}

const typeInto = (input: HTMLInputElement, text: string) => {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const key = (input: HTMLInputElement, k: string) => {
  input.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  flushSync();
};

describe("ProjectStep", () => {
  it("says the list failed, in each provider's word", () => {
    render({ listing: null, loadError: "403 Forbidden." });
    expect(host!.textContent).toContain("Could not list projects");
    expect(host!.textContent).toContain("403 Forbidden. Type the project below instead.");
    void unmount(component!);
    host!.remove();
    const draft = emptyDraft("github");
    render({ listing: null, loadError: "403 Forbidden.", draft });
    expect(host!.textContent).toContain("403 Forbidden. Type the repository below instead.");
  });

  it("gives the reason when the server will not list projects", () => {
    render({ listing: { mode: "type_id_or_path", reason: "This token cannot list projects.", suggestion: null } });
    expect(host!.querySelector('[data-slot="type-reason"]')?.textContent).toBe("This token cannot list projects.");
  });

  it("searches with Enter or the button, trimmed, and on no other key", () => {
    const { calls } = render();
    const search = host!.querySelector<HTMLInputElement>("#wizard-project-search")!;
    typeInto(search, " web ");
    key(search, "w");
    key(search, "Enter");
    [...host!.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Search")!.click();
    expect(calls.search).toEqual(["web", "web"]);
  });

  it("shows no branch for a project without one, and says the list was cut short", () => {
    render();
    const row = [...host!.querySelectorAll("label")].find((l) => l.textContent?.includes("acme/web"))!;
    expect(row.querySelectorAll("span")).toHaveLength(1);
    expect(host!.textContent).toContain("More projects than shown; search to narrow the list.");
  });

  it("looks up a typed project with Enter, writing it into the draft as it is typed", () => {
    const { props, calls } = render();
    const input = host!.querySelector<HTMLInputElement>("#wizard-project-input")!;
    typeInto(input, "acme/api");
    expect(props.draft.projectInput).toBe("acme/api");
    key(input, "a");
    expect(calls.resolve).toBe(0);
    key(input, "Enter");
    expect(calls.resolve).toBe(1);
  });
});
