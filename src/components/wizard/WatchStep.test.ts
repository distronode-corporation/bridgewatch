// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount, type ComponentProps } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import WatchStep from "./WatchStep.svelte";
import { emptyDraft, type Draft } from "./model";

/** The watch step on its own: what each control writes into the draft, and the expected-workflow list's states. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(draft: Draft, props: Partial<ComponentProps<typeof WatchStep>> = {}) {
  const onexpect = vi.fn();
  const all = reactive({ draft, errors: {}, onexpect, ...props }) as ComponentProps<typeof WatchStep>;
  host = document.createElement("div");
  document.body.append(host);
  component = mount(WatchStep, { target: host, props: all });
  flushSync();
  return { props: all, onexpect };
}

function github(group: "run" | "commit" = "commit"): Draft {
  const draft = emptyDraft("github");
  draft.group = group;
  draft.project = { ref: "acme/web", path: "acme/web", default_branch: "main" };
  return draft;
}

const $ = <T extends HTMLElement = HTMLElement>(selector: string) => host!.querySelector<T>(selector)!;
const typeInto = (input: HTMLInputElement, text: string) => {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const click = (selector: string) => {
  $(selector).click();
  flushSync();
};

describe("WatchStep", () => {
  it("unticks a source, and asks for the expected workflows again on a commit group", () => {
    const { props, onexpect } = render(github());
    onexpect.mockClear();
    click("#wizard-source-push");
    expect(props.draft.sources).not.toContain("push");
    expect(onexpect).toHaveBeenCalledTimes(1);
  });

  it("does not ask for expected workflows when a source changes on one row per run", () => {
    const { onexpect } = render(github("run"));
    click("#wizard-source-push");
    expect(onexpect).not.toHaveBeenCalled();
  });

  it("turns the commit group off, back to one row per run", () => {
    const { props } = render(github());
    click("#wizard-group-commit");
    expect(props.draft.group).toBe("run");
  });

  it("asks again when the branch changes on a commit group, and not on one row per run", () => {
    const { onexpect } = render(github());
    onexpect.mockClear();
    $<HTMLInputElement>("#wizard-ref").dispatchEvent(new Event("change", { bubbles: true }));
    expect(onexpect).toHaveBeenCalledTimes(1);
    void unmount(component!);
    host!.remove();
    const run = render(github("run"));
    $<HTMLInputElement>("#wizard-ref").dispatchEvent(new Event("change", { bubbles: true }));
    expect(run.onexpect).not.toHaveBeenCalled();
  });

  it("keeps a watch id the user typed when the branch changes", () => {
    const { props } = render(github("run"));
    typeInto($<HTMLInputElement>("#wizard-watch-id"), "mine");
    expect(props.draft.watchIdEdited).toBe(true);
    typeInto($<HTMLInputElement>("#wizard-ref"), "release");
    expect(props.draft.watchId).toBe("mine");
  });

  it("follows the branch in the suggested id only once there is a project", () => {
    const draft = emptyDraft();
    const { props } = render(draft);
    typeInto($<HTMLInputElement>("#wizard-ref"), "release");
    expect(props.draft.watchId).toBe(emptyDraft().watchId);
  });

  it("ticks and unticks an expected workflow", () => {
    const suggestions = { commits: 3, workflows: [{ file: "ci.yml", seen_in: 3 }] };
    const { props } = render(github(), { expectSuggestions: suggestions });
    click('[data-slot="expect"] button');
    expect(props.draft.expect).toEqual(["ci.yml"]);
    click('[data-slot="expect"] button');
    expect(props.draft.expect).toEqual([]);
  });

  it("says it is reading, why it could not, and that the branch has no runs", () => {
    render(github(), { expectLoading: true });
    expect($('[data-slot="expect"]').textContent).toContain("Reading the newest pushes");
    void unmount(component!);
    host!.remove();
    render(github(), { expectError: "403 Forbidden." });
    expect($('[data-slot="expect"] [role="alert"]').textContent).toBe("403 Forbidden.");
    void unmount(component!);
    host!.remove();
    render(github(), { expectSuggestions: { commits: 0, workflows: [] } });
    expect($('[data-slot="expect-none"]')).not.toBeNull();
  });

  it("marks the workflow and watch id fields invalid when the step has errors for them", () => {
    render(github("run"), { errors: { workflow: "Not a workflow file.", watch_id: "The watch needs an id." } });
    expect($("#wizard-workflow").getAttribute("aria-invalid")).toBe("true");
    expect($("#wizard-watch-id").getAttribute("aria-invalid")).toBe("true");
  });

  it("writes the scheduled-watch switch into the draft", () => {
    const { props } = render(emptyDraft());
    const before = props.draft.scheduleWatch;
    click("#wizard-schedule");
    expect(props.draft.scheduleWatch).toBe(!before);
  });
});
