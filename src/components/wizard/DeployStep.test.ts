// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import type { MarkerSuggestions } from "./api";
import DeployStep from "./DeployStep.svelte";
import { emptyDraft, type Draft } from "./model";

/** The deploy-marker step on its own: suggestions, typed names, and what each choice writes into the draft. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

const SUGGESTIONS: MarkerSuggestions = {
  pipeline_id: 42,
  pipeline_url: "https://gitlab.com/acme/web/-/pipelines/42",
  suggestions: [
    { name: "deploy:prod", stage: "deploy", pipeline: "parent", status: "success", score: 9, reasons: ["named deploy"] },
    { name: "release", stage: null, pipeline: "trigger:web", status: "success", score: 5, reasons: ["last stage"] },
  ],
  unread_children: [],
};

function render(props: Partial<ComponentProps<typeof DeployStep>> & { draft?: Draft } = {}) {
  const all = reactive({
    draft: emptyDraft(),
    errors: {},
    suggestions: SUGGESTIONS,
    loading: false,
    loadError: null,
    ...props,
  }) as ComponentProps<typeof DeployStep>;
  host = document.createElement("div");
  document.body.append(host);
  component = mount(DeployStep, { target: host, props: all });
  flushSync();
  return all;
}

const marker = (name: string) => host!.querySelector<HTMLElement>(`[data-marker="${name}"]`);
const custom = () => host!.querySelector<HTMLInputElement>("#wizard-marker-custom")!;
const key = (target: HTMLElement, k: string) => {
  target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  flushSync();
};
const typeCustom = (text: string) => {
  custom().value = text;
  custom().dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};

describe("DeployStep", () => {
  it("says where a child pipeline's suggestion is, and leaves out a missing stage", () => {
    render();
    expect(marker("release")?.querySelector('[data-slot="marker-reason"]')?.textContent?.trim()).toBe(
      "in trigger:web's child; last stage",
    );
    expect(marker("release")?.textContent).not.toContain("deploy");
    expect(marker("deploy:prod")?.querySelector('[data-slot="marker-reason"]')?.textContent?.trim()).toBe("named deploy");
  });

  it("ticks and unticks a suggestion in the draft", () => {
    const props = render();
    const box = () => marker("deploy:prod")!.querySelector<HTMLButtonElement>("button")!;
    box().click();
    flushSync();
    expect(props.draft.markers).toEqual(["deploy:prod"]);
    box().click();
    flushSync();
    expect(props.draft.markers).toEqual([]);
  });

  it("adds a typed job name with Enter, and ignores Enter on an empty box and every other key", () => {
    const draft = emptyDraft();
    draft.noMarker = false;
    const props = render({ suggestions: null, draft });
    key(custom(), "Enter");
    expect(props.draft.markers).toEqual([]);
    typeCustom("deploy:origins");
    key(custom(), "a");
    expect(props.draft.markers).toEqual([]);
    key(custom(), "Enter");
    expect(props.draft.markers).toEqual(["deploy:origins"]);
    expect(custom().value).toBe("");
    // A typed name has no suggestion behind it, so no reason line.
    expect(marker("deploy:origins")?.querySelector('[data-slot="marker-reason"]')).toBeNull();
  });

  it("goes back to markers from No deploy marker", () => {
    const draft = emptyDraft();
    draft.noMarker = true;
    const props = render({ draft });
    const radio = host!.querySelector<HTMLInputElement>('input[type=radio][value="markers"]')!;
    radio.checked = true;
    radio.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    expect(props.draft.noMarker).toBe(false);
  });

  it("says why the latest pipeline could not be read, and that no job in it looks like a deploy", () => {
    render({ suggestions: null, loadError: "404 Not Found." });
    expect(host!.textContent).toContain("Could not read the latest pipeline");
    void unmount(component!);
    host!.remove();
    render({ suggestions: { ...SUGGESTIONS, suggestions: [] } });
    expect(host!.querySelector('[data-slot="no-suggestions"]')?.textContent?.trim()).toBe(
      "No job in the latest pipeline looks like a deploy.",
    );
    void unmount(component!);
    host!.remove();
    render({ suggestions: { ...SUGGESTIONS, pipeline_id: null, suggestions: [] } });
    expect(host!.querySelector('[data-slot="no-suggestions"]')?.textContent?.trim()).toBe(
      "No pipeline on this branch yet, so nothing to suggest.",
    );
  });

  it("names the child pipelines it could not read", () => {
    render({ suggestions: { ...SUGGESTIONS, unread_children: ["trigger:ios", "trigger:android"] } });
    expect(host!.textContent).toContain("Could not read the child pipelines of trigger:ios, trigger:android.");
  });
});
