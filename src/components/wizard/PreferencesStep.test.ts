// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Distronode Corporation

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { reactive } from "../../lib/__tests__/props.svelte";
import { emptyDraft, type Draft } from "./model";
import PreferencesStep from "./PreferencesStep.svelte";

/** The preferences step on its own: every switch and the poll speed write into the draft. */

let host: HTMLDivElement | null = null;
let component: Record<string, unknown> | null = null;

afterEach(() => {
  if (component) void unmount(component);
  component = null;
  host?.remove();
  host = null;
});

function render(draft: Draft) {
  const props = reactive({ draft, errors: {} });
  host = document.createElement("div");
  document.body.append(host);
  component = mount(PreferencesStep, { target: host, props });
  flushSync();
  return props;
}

describe("PreferencesStep", () => {
  it("writes the notification and launch switches into the draft", () => {
    const props = render(emptyDraft());
    const before = { ...props.draft.notify };
    host!.querySelector<HTMLButtonElement>("#wizard-notify-finished")!.click();
    host!.querySelector<HTMLButtonElement>("#wizard-launch")!.click();
    flushSync();
    expect(props.draft.notify.finished).toBe(!before.finished);
    expect(props.draft.launchAtLogin).toBe(!emptyDraft().launchAtLogin);
  });

  it("keeps a speed from the file that is not one of the choices, and writes the one picked", () => {
    const draft = emptyDraft();
    draft.liveSecs = 7;
    const props = render(draft);
    const select = host!.querySelector<HTMLSelectElement>("#wizard-live-secs")!;
    expect(select.options[0].textContent).toBe("Every 7 s (current)");
    const picked = select.options[1].value;
    select.value = picked;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    expect(props.draft.liveSecs).toBe(Number(picked));
    // A speed from the list needs no extra "current" entry.
    expect([...select.options].some((o) => o.textContent?.includes("(current)"))).toBe(false);
  });
});
